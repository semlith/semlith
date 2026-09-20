//! Images, end to end: indexed into a second vector space, found by a sentence
//! describing them, and removed when their file is forgotten.
//!
//! These drive a real store and download CLIP on first run, so:
//!
//! ```sh
//! cargo test --test image -- --ignored
//! ```
//!
//! The fixtures are drawn here rather than committed: four flat shapes, which
//! is enough for CLIP to tell apart and small enough that the corpus costs
//! nothing. What is asserted is the wiring — a row and a vector per image, the
//! pixel size, the fused ranking, and the eviction — not CLIP's own accuracy,
//! which is its and not semlith's.

use semlith::Semlith;
use std::io::Cursor;
use std::path::Path;

/// A flat shape on white, saved in whichever format the extension names.
fn shape(dir: &Path, name: &str, colour: [u8; 3], kind: Shape) {
    let (w, h) = (224u32, 224u32);
    let mut img = image::RgbImage::from_pixel(w, h, image::Rgb([255, 255, 255]));
    for y in 0..h {
        for x in 0..w {
            let inside = match kind {
                Shape::Circle => {
                    let (dx, dy) = (x as f32 - 112.0, y as f32 - 112.0);
                    dx * dx + dy * dy < 90.0 * 90.0
                }
                Shape::Square => (30..194).contains(&x) && (30..194).contains(&y),
                Shape::Triangle => {
                    let t = y as f32 / h as f32;
                    let half = t * 100.0;
                    (x as f32 - 112.0).abs() < half && y > 30
                }
            };
            if inside {
                img.put_pixel(x, y, image::Rgb(colour));
            }
        }
    }
    let path = dir.join(name);
    image::DynamicImage::ImageRgb8(img)
        .save(&path)
        .unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
}

enum Shape {
    Circle,
    Square,
    Triangle,
}

/// The corpus every test here indexes: three images in three formats, and two
/// text files that have nothing to do with any of them.
fn corpus() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    shape(dir.path(), "red-circle.png", [220, 20, 20], Shape::Circle);
    shape(dir.path(), "blue-square.jpg", [20, 40, 220], Shape::Square);
    shape(
        dir.path(),
        "green-triangle.webp",
        [20, 170, 60],
        Shape::Triangle,
    );
    std::fs::write(
        dir.path().join("notes.md"),
        "# Notes\n\nSharded vector indexes, quantisation and the write-ahead log.\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("parse.rs"),
        "fn main() { println!(\"tokenising a source file\"); }\n",
    )
    .unwrap();
    dir
}

fn open(store: &Path) -> Semlith {
    let mut s = Semlith::open(store, None).unwrap();
    s.quiet = true;
    s
}

/// One row and one vector per image, with the real pixel size, and the text
/// half untouched beside it.
#[test]
#[ignore = "downloads CLIP and an embedding model on first run"]
fn indexing_records_a_row_and_a_vector_for_every_image() {
    let corpus = corpus();
    let store = tempfile::tempdir().unwrap();
    let mut s = open(store.path());
    let report = s
        .index_paths(&[corpus.path().to_path_buf()], |_, _| {})
        .unwrap();

    assert_eq!(report.images, 3, "three images were indexed");
    assert!(report.chunks >= 2, "the text files were chunked as before");

    let rows: Vec<(String, u32, u32)> = s
        .db()
        .prepare("SELECT f.path, i.width, i.height FROM images i JOIN files f ON f.id = i.file_id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(rows.len(), 3);
    for (path, width, height) in &rows {
        assert_eq!((*width, *height), (224, 224), "{path} has the wrong size");
    }
    assert_eq!(s.image_count().unwrap(), 3);

    // The second vector space is a directory beside the text one.
    assert!(
        store.path().join(semlith::image::INDEX_DIR).exists(),
        "no image index was written"
    );
}

/// The point of the feature: a sentence describing a picture finds the picture,
/// above text that has nothing to do with it.
#[test]
#[ignore = "downloads CLIP and an embedding model on first run"]
fn a_sentence_finds_the_image_it_describes() {
    let corpus = corpus();
    let store = tempfile::tempdir().unwrap();
    let mut s = open(store.path());
    s.index_paths(&[corpus.path().to_path_buf()], |_, _| {})
        .unwrap();

    for (query, wanted) in [
        ("a red circle", "red-circle.png"),
        ("a blue square", "blue-square.jpg"),
        ("a green triangle", "green-triangle.webp"),
    ] {
        let hits = s.search(query, 5).unwrap();
        let top = hits
            .first()
            .unwrap_or_else(|| panic!("{query} found nothing"));
        assert!(
            top.path.ends_with(wanted),
            "{query} put {} first, not {wanted}: {:?}",
            top.path,
            hits.iter().map(|h| h.path.as_str()).collect::<Vec<_>>()
        );
        // An image hit carries its pixel size where a chunk carries a line
        // range, and says which list found it.
        let pixels = top.image.expect("an image hit carries its pixel size");
        assert_eq!((pixels.width, pixels.height), (224, 224));
        assert_eq!(top.start_line, 0, "an image has no line range");
        assert!(top.text.is_empty(), "an image has no excerpt");
        assert!(top.lists.contains(&"image"), "{:?}", top.lists);
    }
}

/// And the other way: a query about the text keeps the text first. A weak image
/// match is a candidate, not an answer.
#[test]
#[ignore = "downloads CLIP and an embedding model on first run"]
fn a_query_about_the_text_still_ranks_the_text_first() {
    let corpus = corpus();
    let store = tempfile::tempdir().unwrap();
    let mut s = open(store.path());
    s.index_paths(&[corpus.path().to_path_buf()], |_, _| {})
        .unwrap();

    let hits = s
        .search("sharded vector indexes and quantisation", 5)
        .unwrap();
    let top = hits.first().expect("the text is there");
    assert!(
        top.image.is_none(),
        "an image outranked the text it has nothing to do with: {}",
        top.path
    );
}

/// Forgetting an image takes its row and its vector, and leaves every other
/// image where it was.
#[test]
#[ignore = "downloads CLIP and an embedding model on first run"]
fn forgetting_an_image_removes_its_row_and_its_vector() {
    let corpus = corpus();
    let store = tempfile::tempdir().unwrap();
    let mut s = open(store.path());
    s.index_paths(&[corpus.path().to_path_buf()], |_, _| {})
        .unwrap();

    let (chunks, images) = s.forget(&corpus.path().join("red-circle.png")).unwrap();
    assert_eq!((chunks, images), (0, 1), "an image has no chunks to remove");
    assert_eq!(s.image_count().unwrap(), 2);

    let hits = s.search("a red circle", 5).unwrap();
    assert!(
        !hits.iter().any(|h| h.path.ends_with("red-circle.png")),
        "the forgotten image is still searchable"
    );
    // The others are untouched.
    let hits = s.search("a blue square", 5).unwrap();
    assert!(hits.iter().any(|h| h.path.ends_with("blue-square.jpg")));
}

/// Re-indexing an edited image replaces its vector rather than adding a second.
#[test]
#[ignore = "downloads CLIP and an embedding model on first run"]
fn re_indexing_an_edited_image_replaces_its_vector() {
    let corpus = corpus();
    let store = tempfile::tempdir().unwrap();
    let mut s = open(store.path());
    s.index_paths(&[corpus.path().to_path_buf()], |_, _| {})
        .unwrap();
    assert_eq!(s.image_count().unwrap(), 3);

    // The same path, different pixels: a square where the circle was.
    shape(
        corpus.path(),
        "red-circle.png",
        [20, 40, 220],
        Shape::Square,
    );
    let report = s
        .index_paths(&[corpus.path().to_path_buf()], |_, _| {})
        .unwrap();
    assert_eq!(report.images, 1, "only the edited image was re-embedded");
    assert_eq!(s.image_count().unwrap(), 3, "a second row was written");

    let size: (u32, u32) = s
        .db()
        .query_row(
            "SELECT i.width, i.height FROM images i JOIN files f ON f.id = i.file_id \
             WHERE f.path LIKE '%red-circle.png'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(size, (224, 224));
}

/// A store with no image never asks for CLIP, which is what keeps a corpus of
/// source code from downloading a vision model it will never use.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_store_with_no_image_never_reaches_for_clip() {
    let corpus = tempfile::tempdir().unwrap();
    std::fs::write(
        corpus.path().join("only.md"),
        "# Only text\n\nNothing here is a picture.\n",
    )
    .unwrap();
    let store = tempfile::tempdir().unwrap();
    let mut s = open(store.path());
    let report = s
        .index_paths(&[corpus.path().to_path_buf()], |_, _| {})
        .unwrap();
    assert_eq!(report.images, 0);

    // Searching is the other half: the image list is skipped entirely when the
    // store holds no image, so no second model is loaded to answer.
    let hits = s.search("anything at all", 5).unwrap();
    assert!(hits.iter().all(|h| h.image.is_none()));
    assert_eq!(s.image_count().unwrap(), 0);
}

/// The header is enough to size an image, and a file that is not one is
/// skipped rather than failing the run.
#[test]
fn a_file_that_is_not_an_image_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("broken.png");
    std::fs::write(&path, b"this is not a png at all").unwrap();
    assert!(semlith::image::is_image(&path));
    assert_eq!(
        semlith::image::dimensions(b"this is not a png at all"),
        None
    );

    let mut png = Vec::new();
    image::DynamicImage::ImageRgb8(image::RgbImage::new(7, 11))
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    assert_eq!(semlith::image::dimensions(&png), Some((7, 11)));
}

/// A decoder allocates for the dimensions in the header before it has read the
/// pixels, so a header claiming sixty thousand by sixty thousand is a few
/// hundred bytes on disk and fourteen gigabytes in memory.
#[test]
fn an_image_whose_header_claims_too_many_pixels_is_refused_before_it_is_decoded() {
    // A real GIF, rewritten so its logical screen claims a size nothing should
    // decode. GIF rather than PNG because its header carries no checksum: the
    // point is a file whose *header* lies, which a PNG cannot do without also
    // failing its CRC and being rejected for the wrong reason.
    let mut gif = Vec::new();
    image::DynamicImage::ImageRgb8(image::RgbImage::new(4, 4))
        .write_to(&mut std::io::Cursor::new(&mut gif), image::ImageFormat::Gif)
        .unwrap();
    // Bytes 6..10 are the logical screen width and height, little-endian.
    let huge = 60_000u16.to_le_bytes();
    gif[6..8].copy_from_slice(&huge);
    gif[8..10].copy_from_slice(&huge);

    let why = semlith::image::too_large(&gif)
        .expect("a 60000x60000 header should be past the pixel budget");
    assert!(
        why.contains("60000") && why.contains("pixels"),
        "the refusal does not say how big it claimed to be: {why}"
    );

    // And an ordinary image is not refused.
    let mut small = Vec::new();
    image::DynamicImage::ImageRgb8(image::RgbImage::new(64, 64))
        .write_to(
            &mut std::io::Cursor::new(&mut small),
            image::ImageFormat::Png,
        )
        .unwrap();
    assert_eq!(semlith::image::too_large(&small), None);
}

/// The measurement behind #122: what each hit scored and which lists found it.
///
/// Kept rather than deleted after the fix, because the fix is a ranking
/// change and the only way to tell whether a later one moved it is to print
/// the same three numbers again.
///
/// ```sh
/// cargo test --test image -- --ignored what_an_image_query_scores --nocapture
/// ```
#[test]
#[ignore = "downloads CLIP and an embedding model on first run"]
fn what_an_image_query_scores() {
    let corpus = corpus();
    let store = tempfile::tempdir().unwrap();
    let mut s = open(store.path());
    s.index_paths(&[corpus.path().to_path_buf()], |_, _| {})
        .unwrap();
    for query in ["a red circle", "a blue square", "a green triangle"] {
        println!("\n{query}");
        for hit in s.search(query, 5).unwrap() {
            let name = std::path::Path::new(&hit.path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            println!(
                "  {:<24} {:.6}  {}",
                name,
                hit.score,
                hit.lists.join("+")
            );
        }
    }
}
