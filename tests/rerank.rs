//! The rescoring stage: it reorders, it is deterministic, and it can be
//! switched off to nothing.

use semlith::Semlith;
use std::path::Path;

fn write(dir: &Path, name: &str, text: &str) {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, text).unwrap();
}

/// A corpus where the word the query uses is spread across several files, so
/// the order is decided by reading rather than by matching.
fn corpus(dir: &Path) {
    write(
        dir,
        "retry.rs",
        "/// Retry a failed request with an exponential backoff.\n\
         pub fn retry(attempts: u32) -> Result<()> {\n    backoff(attempts)\n}\n",
    );
    write(
        dir,
        "notes.md",
        "# Notes\n\nThe word retry appears here and nowhere useful.\n",
    );
    write(
        dir,
        "backoff.rs",
        "/// How long to wait before the next attempt, doubling each time.\n\
         pub fn backoff(attempts: u32) -> Result<()> {\n    sleep(1 << attempts)\n}\n",
    );
}

fn order_of(semlith: &mut Semlith, query: &str) -> Vec<String> {
    semlith
        .search(query, 8)
        .unwrap()
        .into_iter()
        .map(|hit| hit.path)
        .collect()
}

/// With the stage off, the order is the fused order — exactly, not nearly.
///
/// This is what makes the stage's contribution one number: the same binary,
/// the same store, one environment variable, and the difference between the
/// two answers is the whole of what the cross-encoder did.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn the_stage_switches_off_to_the_fused_order() {
    let work = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    corpus(work.path());

    let mut semlith = Semlith::open(store.path(), None).unwrap();
    semlith.quiet = true;
    semlith
        .index_paths(&[work.path().to_path_buf()], |_, _| {})
        .unwrap();

    // Off is the default, so this is the shipped path.
    assert!(!semlith::rerank::enabled());
    let off = order_of(&mut semlith, "how does the client wait between attempts");
    let again = order_of(&mut semlith, "how does the client wait between attempts");
    assert_eq!(off, again, "the fused order is not deterministic");

    // SAFETY: one test process, one variable, read inside these calls only.
    unsafe { std::env::set_var(semlith::rerank::RERANK_ENV, "on") };
    assert!(semlith::rerank::enabled());
    let on = order_of(&mut semlith, "how does the client wait between attempts");
    let on_again = order_of(&mut semlith, "how does the client wait between attempts");
    assert_eq!(
        on, on_again,
        "the rescored order is not deterministic for one query over one store"
    );

    // It ran. A stage that failed to load its model, or skipped every
    // candidate, would leave an order identical to the fused one and no badge
    // — which is exactly what a silent no-op looks like from outside.
    let badged = semlith
        .search("how does the client wait between attempts", 8)
        .unwrap()
        .into_iter()
        .filter(|hit| hit.lists.contains(&"rerank"))
        .count();
    assert!(
        badged > 0,
        "no hit carries the rerank badge: the stage did not run"
    );

    // Present or absent, the stage never loses or invents a candidate: it
    // reorders what fusion already found.
    let mut sorted_off = off.clone();
    let mut sorted_on = on.clone();
    sorted_off.sort();
    sorted_on.sort();
    assert_eq!(
        sorted_off, sorted_on,
        "the rescoring stage changed which chunks came back, not only their order"
    );
}

/// The model itself: it loads from the pinned revision, and it puts the
/// passage that answers the question in front of the one that merely shares
/// its vocabulary.
#[test]
#[ignore = "downloads the cross-encoder on first run"]
fn the_cross_encoder_reads_the_pair() {
    let cache = semlith::model_cache_dir().unwrap();
    let mut model = semlith::rerank::load(&cache, true).expect("the rescoring model loads");
    let texts = vec![
        "The word retry appears here and nowhere useful.".to_string(),
        "How long to wait before the next attempt, doubling each time.".to_string(),
    ];
    let order = semlith::rerank::order(&mut model, "how long does it wait between retries", &texts)
        .expect("the pair is scored");
    assert_eq!(
        order.first().copied(),
        Some(1),
        "the cross-encoder ranked the vocabulary match above the answer"
    );
    assert!(
        semlith::rerank::cached(&cache),
        "the pin is not in the cache"
    );
}
