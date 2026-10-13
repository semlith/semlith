//! A store built by semlith 0.38.1, opened by this binary.
//!
//! 0.39.0 moved to turbovec 1.1, which reads the v7 vector files 0.38.1 wrote
//! and writes v8 ones the next time it saves a file. A semlith before 0.39.0
//! cannot load v8, so the first write marks the store format 5 before anything
//! is saved, and an older binary then refuses the store by name rather than
//! failing on a vector file. These tests hold the three halves of that: a write
//! moves the store to format 5 and its vectors to v8, a read moves nothing, and
//! search answers the same across the move.
//!
//! `tests/fixtures/store-0.38.1` is that store as 0.38.1 left it: twelve
//! one-chunk files, one vector shard (`TV7\0`), `format_version` 4, embedded on
//! the CPU lane so the vectors carry the `int8-cpu` variant any machine reads.
//! Its rows name the files where they were indexed, under
//! `/private/tmp/claude-501/fixture-0.38.1/corpus`. Nothing here reads those
//! files, and `forget` takes a path that no longer exists as the key it is, so
//! the same paths serve on every OS.
//!
//! The file is named for the store, not `upgrade.rs`: that one is
//! `semlith upgrade`, the binary replacing itself.

use semlith::Semlith;
use std::fs;
use std::path::{Path, PathBuf};

/// Where 0.38.1 indexed the fixture's corpus.
const CORPUS: &str = "/private/tmp/claude-501/fixture-0.38.1/corpus";

/// How many files, and so chunks, the fixture holds.
const FILES: usize = 12;

/// turbovec's bit width for every semlith store. Private to the crate, and the
/// same since the first sharded format, so it is spelled out here.
const BIT_WIDTH: usize = 4;

/// The fixture, copied: a test that writes must never write the checked-in one.
fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/store-0.38.1"),
        dir.path(),
    );
    dir
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

fn open(dir: &Path) -> Semlith {
    let mut s = Semlith::open(dir, None).unwrap();
    s.quiet = true;
    s
}

/// Every vector file of the store, with its first four bytes.
fn magics(dir: &Path) -> Vec<(PathBuf, [u8; 4])> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir.join("index")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "tvim") {
            let bytes = fs::read(&path).unwrap();
            out.push((path, bytes[..4].try_into().unwrap()));
        }
    }
    assert!(!out.is_empty(), "no vector files under {}", dir.display());
    out
}

fn chunk_ids(s: &Semlith) -> Vec<u64> {
    let mut stmt = s.db().prepare("SELECT id FROM chunks ORDER BY id").unwrap();
    stmt.query_map([], |r| r.get::<_, i64>(0).map(|id| id as u64))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// The fixture is what the tests below say it is, so a regenerated one that
/// is not still fails here first rather than passing them vacuously.
#[test]
fn the_fixture_is_a_format_4_store_with_v7_vectors() {
    let dir = fixture();
    let s = open(dir.path());
    assert_eq!(
        semlith::store::format(s.db()).unwrap(),
        semlith::store::CODE_CONTEXT
    );
    assert_eq!(s.stats().unwrap().0 as usize, FILES);
    for (path, magic) in magics(dir.path()) {
        assert_eq!(&magic, b"TV7\0", "{}", path.display());
    }
}

/// The first write marks the store format 5 and the shard it saves is v8, and
/// every vector the rows still name loads from it.
#[test]
fn a_write_moves_the_store_to_format_5_and_v8() {
    let dir = fixture();
    let mut s = open(dir.path());
    let (chunks, _) = s.forget(&Path::new(CORPUS).join("chess.md")).unwrap();
    assert_eq!(chunks, 1, "the fixture's chess.md is one chunk");

    assert_eq!(
        semlith::store::format(s.db()).unwrap(),
        semlith::store::FORMAT_VERSION
    );
    assert_eq!(semlith::store::FORMAT_VERSION, semlith::store::TURBOVEC_V8);
    assert!(
        magics(dir.path()).iter().any(|(_, m)| m == b"TV8\0"),
        "no vector file was saved as v8: {:?}",
        magics(dir.path())
    );

    let ids = chunk_ids(&s);
    assert_eq!(ids.len(), FILES - 1);
    let dim = s.dim();
    drop(s);
    // Read back from disk, not from the open index: the claim is about the
    // files an older or a later process will load.
    let on_disk = semlith::index::ids_on_disk(dir.path(), dim, BIT_WIDTH).unwrap();
    assert_eq!(on_disk, ids);

    // And the store reopens at format 5, its rows and vectors still agreeing.
    let s = open(dir.path());
    assert_eq!(
        semlith::store::format(s.db()).unwrap(),
        semlith::store::FORMAT_VERSION
    );
    assert_eq!(chunk_ids(&s), on_disk);
}

/// A read stamps nothing: opening a 0.38.1 store and asking it about itself
/// must leave it one 0.38.1 can still open. Stamping on read would lock every
/// user out of rolling back the moment they ran `semlith stats`.
#[test]
fn a_read_leaves_the_store_at_format_4() {
    let dir = fixture();
    let before = magics(dir.path());
    {
        let s = open(dir.path());
        assert_eq!(s.stats().unwrap().0 as usize, FILES);
        assert_eq!(chunk_ids(&s).len(), FILES);
    }
    let s = open(dir.path());
    assert_eq!(
        semlith::store::format(s.db()).unwrap(),
        semlith::store::CODE_CONTEXT
    );
    assert_eq!(magics(dir.path()), before);
}

/// Search answers the same on either side of the move: v7 vectors read by
/// turbovec 1.1, then the v8 file it wrote back.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn search_finds_the_same_file_before_and_after_the_move() {
    let dir = fixture();
    let mut s = open(dir.path());
    let top = |s: &mut Semlith| {
        s.search("apiary hives queen bees honey", 3).unwrap()[0]
            .path
            .clone()
    };

    assert!(top(&mut s).ends_with("beekeeping.txt"), "{}", top(&mut s));
    s.forget(&Path::new(CORPUS).join("volcano.txt")).unwrap();
    assert_eq!(
        semlith::store::format(s.db()).unwrap(),
        semlith::store::FORMAT_VERSION
    );
    assert!(top(&mut s).ends_with("beekeeping.txt"), "{}", top(&mut s));

    drop(s);
    let mut s = open(dir.path());
    assert!(top(&mut s).ends_with("beekeeping.txt"), "{}", top(&mut s));
}
