//! The second stage: a cross-encoder that reads the query and a candidate
//! together and says how well they answer each other.
//!
//! The fusion ranks by where a chunk placed in lists that never saw the query
//! and the chunk side by side — a vector is compared against a vector, and FTS5
//! against terms. A cross-encoder reads both at once, which is what makes it
//! better at ordering and far too slow to apply to everything.
//!
//! So it applies to [`TOP`] candidates and not to the fifty the fusion considers.
//! Fifty passes is 50-150 ms on the reference laptop; ten is a few tens of
//! milliseconds, and the questions this was built for are the ones where the
//! right span is already inside the top eight and sitting third or fifth. The
//! measurement that prompted it: twenty questions of seventy-seven were found
//! inside the top eight and outside the top three.

use anyhow::{Context, Result};
use fastembed::{RerankInitOptions, RerankerModel, TextRerank};
use std::path::PathBuf;

/// How many of the fused candidates the cross-encoder reads.
///
/// Ten rather than the fifty the contract first asked for. The cost is linear
/// in this number and the gain is not: a span that the fusion put fortieth is
/// not one the reranker was going to promote to first.
pub const TOP: usize = 10;

/// The smallest English cross-encoder fastembed offers, at 37.8M parameters.
///
/// `BGERerankerBase` is the crate's default and is an order of magnitude
/// larger; a reranker that doubles what a user downloads to install semlith is
/// not one this crate should reach for by default.
const MODEL: RerankerModel = RerankerModel::JINARerankerV1TurboEn;

/// Load the cross-encoder, into the same cache every other model uses.
///
/// ponytail: fastembed fetches this one itself, so it is not digest-pinned the
/// way `embed::GRANITE_FILES` pins the embedding model. Pinning it means
/// copying the `UserDefinedRerankingModel` path the way `load_granite` copies
/// the embedding one, and recording the digests — worth doing before this is
/// the default on a machine that cannot check what it downloaded, and not
/// worth blocking the measurement it exists to produce.
pub fn load(cache_dir: PathBuf, quiet: bool) -> Result<TextRerank> {
    if crate::embed::airgap() && !is_cached(&cache_dir) {
        anyhow::bail!(
            "{} is set and no reranker is cached at {} — pre-seed it with \
             SEMLITH_MODEL_CACHE on a connected machine, or run without reranking",
            crate::embed::AIRGAP_ENV,
            cache_dir.display()
        );
    }
    crate::embed::link_runtime()?;
    TextRerank::try_new(
        RerankInitOptions::new(MODEL)
            .with_cache_dir(cache_dir)
            .with_show_download_progress(!quiet),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
    .context("loading the reranker")
}

/// Whether this cache already holds the reranker, so `--airgap` can tell a
/// machine that has one from a machine that would have to fetch it.
fn is_cached(cache_dir: &std::path::Path) -> bool {
    let Ok(entries) = std::fs::read_dir(cache_dir) else {
        return false;
    };
    entries.flatten().any(|e| {
        e.file_name()
            .to_string_lossy()
            .to_lowercase()
            .contains("rerank")
    })
}
