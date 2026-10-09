//! A run's account of itself, end to end: what the scan counts is what the
//! run writes, the expected total converges on the rows written without ever
//! resetting, the share done only climbs, and the phases come in order.
//!
//! Ignored by default: the first run downloads the embedding model.
//!
//! ```sh
//! cargo test --test progress -- --ignored --test-threads=1
//! ```

use semlith::progress::Phase;
use semlith::{FileOutcome, Semlith};

/// Prose, code, a dense JSON file and one much larger than the rest: the
/// shapes that made rc.4's first figures swing.
fn corpus(at: &std::path::Path) {
    for i in 0..12 {
        std::fs::write(
            at.join(format!("note_{i}.md")),
            format!(
                "# Note {i}\n\n{}\n",
                "Ownership, retries and backoff. ".repeat(40 + i * 7)
            ),
        )
        .unwrap();
        std::fs::write(
            at.join(format!("code_{i}.rs")),
            format!(
                "/// Doc {i}\npub fn f{i}(x: u32) -> u32 {{\n{}    x\n}}\n",
                "    let y = x.wrapping_mul(3);\n".repeat(30 + i)
            ),
        )
        .unwrap();
    }
    let rows: Vec<String> = (0..400)
        .map(|i| {
            format!(
                "{{\"id\":{i},\"name\":\"row {i}\",\"tags\":[\"a\",\"b\",\"c\"],\"score\":{}}}",
                i * 3
            )
        })
        .collect();
    std::fs::write(at.join("data.jsonl"), rows.join("\n")).unwrap();
    std::fs::write(
        at.join("big.md"),
        format!(
            "# Big\n\n{}",
            "A long paragraph about indexes, locks and their costs. ".repeat(3_000)
        ),
    )
    .unwrap();
}

#[test]
#[ignore = "downloads an embedding model on first run"]
fn the_account_converges_climbs_and_says_its_phases() {
    // SAFETY: read only by this test binary's index pass, on this thread.
    unsafe { std::env::set_var(semlith::accel::ACCEL_ENV, "cpu") };
    let dir = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    corpus(dir.path());
    let roots = [dir.path().to_path_buf()];

    let mut s = Semlith::open(store.path(), None).unwrap();
    s.quiet = true;
    let plan = s.plan(&roots).unwrap();
    assert!(plan.chunks > 0, "the plan counts chunks: {plan:?}");
    s.planned = Some(std::sync::Arc::new(plan.counts.clone()));

    let mut last_progress = 0.0;
    let mut expected = Vec::new();
    let mut phases: Vec<Phase> = Vec::new();
    let report = s
        .index_paths(&roots, |_, p| {
            assert!(
                p.progress >= last_progress,
                "{} after {last_progress}",
                p.progress
            );
            assert!(p.progress < 1.0);
            last_progress = p.progress;
            expected.push((p.rows as u64, p.expected_chunks));
            if p.outcome == FileOutcome::Phase && phases.last() != Some(&p.phase) {
                phases.push(p.phase);
            }
        })
        .unwrap();

    // The expected total ends exactly at the rows written, and every figure
    // on the way was at least the rows written then.
    let (rows, last) = *expected.last().unwrap();
    assert_eq!(rows, report.rows as u64);
    assert_eq!(last, rows, "converged on the rows written");
    assert!(expected.iter().all(|(r, e)| e >= r));
    // The scan's count, made without the symbols the run cuts code at, is
    // within a tenth of what the run wrote.
    let off = (plan.chunks as f64 - rows as f64).abs() / rows as f64;
    assert!(off < 0.10, "plan {} vs rows {rows}", plan.chunks);
    // Phases in the order a run goes through them.
    let at = |phase: Phase| phases.iter().position(|p| *p == phase);
    assert!(at(Phase::Save).is_some(), "{phases:?}");
    if let (Some(drain), Some(save)) = (at(Phase::Drain), at(Phase::Save)) {
        assert!(drain < save, "{phases:?}");
    }
}
