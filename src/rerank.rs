//! The rescoring stage: a cross-encoder over the head of the fused list.
//!
//! Fusion ranks by position. A chunk is high because two lists put it high,
//! not because anything read the query and the chunk together — and the
//! measurement that made this release necessary says exactly that: on the
//! development set, twenty questions had a satisfying span inside the top
//! eight and outside the top three, which is a pure ordering problem over
//! candidates already in hand.
//!
//! A cross-encoder is what reads the pair. It embeds nothing, stores nothing
//! and adds no candidates: it scores `(query, text)` together and reorders
//! what the anchor and reach stages already found. That is why it sits last
//! and why it can be switched off at run time without changing anything else
//! — with the stage off, the order is the fused order exactly, which is what
//! makes its contribution one number rather than an opinion.
//!
//! The model is pinned the way the embedding model is: a repository, a commit,
//! and a SHA-256 for every file. It is fetched by `semlith setup` and never at
//! query time, so a machine with no network answers with the stage on.

use anyhow::{Context, Result};
use fastembed::{
    OnnxSource, RerankInitOptionsUserDefined, TextRerank, TokenizerFiles, UserDefinedRerankingModel,
};
use std::path::{Path, PathBuf};

/// The repository the cross-encoder comes from.
///
/// Apache-2.0, 37 M parameters, English. Chosen over a larger reranker for the
/// same reason the embedding model is small: this runs on a laptop inside a
/// query, and a model that doubles the answer's latency is a model nobody
/// leaves on.
pub const RERANK_REPO: &str = "jinaai/jina-reranker-v1-turbo-en";

/// The commit those files are read at. A branch is a name somebody else
/// controls; a commit is the bytes this release was built against.
pub const RERANK_REVISION: &str = "b8c14f4e723d9e0aab4732a7b7b93741eeeb77c2";

/// What `stats`, `doctor` and the record call it.
pub const RERANK_NAME: &str = "jina-reranker-v1-turbo-en-int8";

/// The quantized graph, so the stage costs 38 MB on disk rather than 150.
pub const RERANK_ONNX: &str = "onnx/model_quantized.onnx";

/// Every file semlith fetches from [`RERANK_REPO`], with its SHA-256.
pub const RERANK_FILES: &[(&str, &str)] = &[
    (
        "tokenizer.json",
        "0046da43cc8c424b317f56b092b0512aaaa65c4f925d2f16af9d9eeb4d0ef902",
    ),
    (
        "config.json",
        "e050ff6a15ae9295e84882fa0e98051bd8754856cd5201395ebf00ce9f2d609b",
    ),
    (
        "special_tokens_map.json",
        "06e405a36dfe4b9604f484f6a1e619af1a7f7d09e34a8555eb0b77b66318067f",
    ),
    (
        "tokenizer_config.json",
        "d291c6652d96d56ffdbcf1ea19d9bae5ed79003f7648c627e725a619227ce8fa",
    ),
    (
        RERANK_ONNX,
        "3defdef1ae34e119bd704216087743e79665934c96aebabcb6077c239dc3ae66",
    ),
];

/// How many candidates the cross-encoder reads.
///
/// The stage is quadratic in nothing and linear in this, and the population it
/// exists for — a satisfying span at rank four to eight — is inside the first
/// handful. Fifty is deep enough to hold every one of them and shallow enough
/// to stay inside the latency budget on the reference laptop.
pub const RERANK_DEPTH: usize = 50;

/// Turn the stage off for a run: `SEMLITH_RERANK=off`.
///
/// An ablation switch, not a preference. What the stage bought is measured by
/// running the same questions with it on and off, and a stage that can only be
/// measured by rebuilding the binary is a stage nobody measures.
pub const RERANK_ENV: &str = "SEMLITH_RERANK";

/// Whether the stage is on for this process.
#[must_use]
pub fn enabled() -> bool {
    !matches!(
        std::env::var(RERANK_ENV).ok().as_deref(),
        Some("off" | "0" | "false")
    )
}

/// Load the cross-encoder from the model cache, fetching it if it is absent.
///
/// The same shape as [`crate::embed::Model::load`]: pinned revision, a digest
/// per file, and the cache directory the rest of semlith uses.
pub fn load(cache_dir: &Path, quiet: bool) -> Result<TextRerank> {
    crate::embed::link_runtime()?;
    crate::embed::check_cache_dir(cache_dir)?;
    crate::embed::refuse_if_airgapped(RERANK_NAME)?;

    let repo = hf_hub::api::sync::ApiBuilder::new()
        .with_cache_dir(cache_dir.to_path_buf())
        .with_progress(!quiet)
        .build()
        .context("building the Hugging Face client")?
        .repo(hf_hub::Repo::with_revision(
            RERANK_REPO.to_string(),
            hf_hub::RepoType::Model,
            RERANK_REVISION.to_string(),
        ));

    let fetch = |name: &str| -> Result<Vec<u8>> {
        let path = repo
            .get(name)
            .with_context(|| format!("fetching {name} from {RERANK_REPO}"))?;
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let expected = RERANK_FILES
            .iter()
            .find(|(file, _)| *file == name)
            .map(|(_, digest)| *digest)
            .with_context(|| format!("{name} is not a file semlith pins a digest for"))?;
        crate::embed::verify(name, &bytes, expected)?;
        Ok(bytes)
    };

    let tokenizer_files = TokenizerFiles {
        tokenizer_file: fetch("tokenizer.json")?,
        config_file: fetch("config.json")?,
        special_tokens_map_file: fetch("special_tokens_map.json")?,
        tokenizer_config_file: fetch("tokenizer_config.json")?,
    };
    let model =
        UserDefinedRerankingModel::new(OnnxSource::Memory(fetch(RERANK_ONNX)?), tokenizer_files);
    let options = RerankInitOptionsUserDefined::default()
        .with_max_length(512)
        .with_intra_threads(crate::embed::embed_threads());
    TextRerank::try_new_from_user_defined(model, options)
        .map_err(|e| anyhow::anyhow!("loading {RERANK_NAME}: {e}"))
}

/// Whether the cross-encoder's files are already in the cache.
///
/// `doctor` and `stats` ask this: a stage that silently turns itself off
/// because a file is missing is the silence this project keeps finding.
#[must_use]
pub fn cached(cache_dir: &Path) -> bool {
    let Some(dir) = snapshot(cache_dir) else {
        return false;
    };
    RERANK_FILES.iter().all(|(name, _)| dir.join(name).exists())
}

fn snapshot(cache_dir: &Path) -> Option<PathBuf> {
    let owner = RERANK_REPO.replace('/', "--");
    let dir = cache_dir
        .join(format!("models--{owner}"))
        .join("snapshots")
        .join(RERANK_REVISION);
    dir.is_dir().then_some(dir)
}

/// The new order of `texts`, best first, as indices into what was passed.
///
/// Indices rather than a reordered list, so the caller keeps whatever it holds
/// beside each candidate — a chunk id, a score, the lists it came from — and
/// nothing has to be rebuilt to be reordered.
pub fn order(model: &mut TextRerank, query: &str, texts: &[String]) -> Result<Vec<usize>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let scored = model
        .rerank(
            query,
            texts.iter().map(String::as_str).collect::<Vec<&str>>(),
            false,
            None,
        )
        .map_err(|e| anyhow::anyhow!("reranking: {e}"))?;
    Ok(scored.into_iter().map(|result| result.index).collect())
}
