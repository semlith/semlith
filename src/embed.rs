//! Which embedding model a store uses, and how to load it.
//!
//! fastembed ships an enum of models it knows how to fetch and configure. The
//! default is not in that enum: granite-embedding-small-english-r2, quantized
//! to int8, has to be assembled by hand from its ONNX graph, its separate
//! weights file, and its tokenizer. [`Model`] is the union of "something
//! fastembed knows" and "the one we assemble ourselves", so a store can record
//! either and read it back.

use anyhow::{Context, Result};
use fastembed::{
    EmbeddingModel, InitOptionsUserDefined, Pooling, TextEmbedding, TextInitOptions,
    TokenizerFiles, UserDefinedEmbeddingModel,
};
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// The Hugging Face repository holding granite's ONNX export.
const GRANITE_REPO: &str = "onnx-community/granite-embedding-small-english-r2-ONNX";

/// int8 rather than fp32: a quarter of the download, and measurably faster on
/// ARM despite the folklore that quantized ONNX never is.
const GRANITE_ONNX: &str = "onnx/model_quantized.onnx";

/// The graph refers to its weights by this exact name. ONNX Runtime matches on
/// the string, so it has to survive the round trip through the loader verbatim.
const GRANITE_WEIGHTS: &str = "onnx/model_quantized.onnx_data";

/// What a store writes into its `model` meta row for granite. Changing this
/// string orphans every store that recorded the old one.
pub const GRANITE_NAME: &str = "granite-embedding-small-english-r2-int8";

const GRANITE_DIM: usize = 384;

/// Cap on ONNX Runtime's intra-op thread pool, overriding the derived value.
pub const THREADS_ENV: &str = "SEMLITH_EMBED_THREADS";

/// A model a store can be built with.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Model {
    /// One of fastembed's own; it handles fetching and configuration.
    Builtin(EmbeddingModel),
    /// granite-embedding-small-english-r2, int8. Assembled here because
    /// fastembed has no entry for it. This is the default for new stores.
    #[default]
    Granite,
}

impl std::fmt::Display for Model {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Model::Granite => f.write_str(GRANITE_NAME),
            Model::Builtin(m) => write!(f, "{m}"),
        }
    }
}

impl FromStr for Model {
    type Err = String;

    /// Stores written by 0.1.0 hold a fastembed enum name such as
    /// `BGESmallENV15`, so that spelling has to keep parsing exactly as before.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == GRANITE_NAME {
            return Ok(Model::Granite);
        }
        EmbeddingModel::from_str(s).map(Model::Builtin)
    }
}

impl Model {
    pub fn dim(&self) -> Result<usize> {
        match self {
            Model::Granite => Ok(GRANITE_DIM),
            Model::Builtin(m) => Ok(TextEmbedding::get_model_info(m)
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .dim),
        }
    }

    /// BGE English models were trained with an asymmetric query instruction;
    /// omitting it measurably costs recall. granite was trained without one,
    /// and adding a prefix it never saw would only add noise.
    pub fn query_text(&self, query: &str) -> String {
        match self {
            Model::Builtin(m) => {
                let name = m.to_string();
                if name.starts_with("BGE") && name.contains("EN") {
                    format!("Represent this sentence for searching relevant passages: {query}")
                } else {
                    query.to_string()
                }
            }
            Model::Granite => query.to_string(),
        }
    }

    pub fn load(
        &self,
        cache_dir: PathBuf,
        max_length: usize,
        quiet: bool,
    ) -> Result<TextEmbedding> {
        match self {
            Model::Builtin(m) => {
                let opts = TextInitOptions::new(m.clone())
                    .with_show_download_progress(!quiet)
                    .with_max_length(max_length)
                    .with_intra_threads(embed_threads())
                    .with_cache_dir(cache_dir);
                TextEmbedding::try_new(opts).map_err(|e| anyhow::anyhow!("{e}"))
            }
            Model::Granite => load_granite(cache_dir, max_length, quiet),
        }
    }
}

fn load_granite(cache_dir: PathBuf, max_length: usize, quiet: bool) -> Result<TextEmbedding> {
    let repo = hf_hub::api::sync::ApiBuilder::new()
        .with_cache_dir(cache_dir)
        .with_progress(!quiet)
        .build()
        .context("building the Hugging Face client")?
        .model(GRANITE_REPO.to_string());

    let fetch = |name: &str| -> Result<Vec<u8>> {
        let path = repo
            .get(name)
            .with_context(|| format!("fetching {name} from {GRANITE_REPO}"))?;
        std::fs::read(&path).with_context(|| format!("reading {}", path.display()))
    };

    let tokenizer_files = TokenizerFiles {
        tokenizer_file: fetch("tokenizer.json")?,
        config_file: fetch("config.json")?,
        special_tokens_map_file: fetch("special_tokens_map.json")?,
        tokenizer_config_file: fetch("tokenizer_config.json")?,
    };

    // fastembed does not export ExternalInitializerFile, so the weights can
    // only be attached through this builder — a struct literal will not compile.
    let model = UserDefinedEmbeddingModel::new(fetch(GRANITE_ONNX)?, tokenizer_files)
        // 1_Pooling/config.json in the source repo sets pooling_mode_cls_token.
        .with_pooling(Pooling::Cls)
        .with_external_initializer(
            Path::new(GRANITE_WEIGHTS)
                .file_name()
                .expect("weights constant has a file name")
                .to_string_lossy()
                .into_owned(),
            fetch(GRANITE_WEIGHTS)?,
        );

    let opts = InitOptionsUserDefined::new()
        .with_max_length(max_length)
        .with_intra_threads(embed_threads());

    TextEmbedding::try_new_from_user_defined(model, opts)
        .map_err(|e| anyhow::anyhow!("loading {GRANITE_NAME}: {e}"))
}

/// How many threads ONNX Runtime should use inside one operator.
///
/// ORT synchronises its threads at every operator boundary, so the slowest
/// thread paces the whole batch. On a CPU with both performance and efficiency
/// cores, a thread scheduled onto an efficiency core drags everything with it:
/// measured on a 4P+4E M1, four threads indexed at 16.5 chunks/s while eight
/// managed only 13.9, and one managed 5.1. Undersubscribing costs far more than
/// oversubscribing, so only heterogeneous machines get a reduced count.
pub fn embed_threads() -> usize {
    if let Ok(raw) = std::env::var(THREADS_ENV)
        && let Ok(n) = raw.parse::<usize>()
        && n > 0
    {
        return n;
    }
    performance_cores().unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    })
}

/// Performance-core count on Apple silicon. `None` everywhere else, where all
/// cores are equal and the total is the right answer.
#[cfg(target_os = "macos")]
fn performance_cores() -> Option<usize> {
    let mut out: i32 = 0;
    let mut len = std::mem::size_of::<i32>();
    let name = c"hw.perflevel0.logicalcpu";
    // SAFETY: name is a NUL-terminated C string, and out/len describe a live
    // i32 whose size sysctlbyname is told about and will not exceed.
    let rc = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            &mut out as *mut i32 as *mut libc::c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (rc == 0 && out > 0).then_some(out as usize)
}

#[cfg(not(target_os = "macos"))]
fn performance_cores() -> Option<usize> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn granite_round_trips_through_its_stored_name() {
        let parsed: Model = GRANITE_NAME.parse().unwrap();
        assert_eq!(parsed, Model::Granite);
        assert_eq!(parsed.to_string(), GRANITE_NAME);
        assert_eq!(parsed.dim().unwrap(), 384);
    }

    #[test]
    fn stores_written_by_0_1_0_still_parse() {
        // The exact spelling a 0.1.0 store holds in its `model` meta row.
        let parsed: Model = "BGESmallENV15".parse().unwrap();
        assert_eq!(parsed, Model::Builtin(EmbeddingModel::BGESmallENV15));
        assert_eq!(parsed.to_string(), "BGESmallENV15");
        assert_eq!(parsed.dim().unwrap(), 384);
    }

    #[test]
    fn nonsense_model_names_are_rejected() {
        assert!("not-a-model".parse::<Model>().is_err());
    }

    #[test]
    fn only_bge_english_gets_the_instruction_prefix() {
        let bge = Model::Builtin(EmbeddingModel::BGESmallENV15);
        assert!(bge.query_text("hi").starts_with("Represent this sentence"));
        assert!(bge.query_text("hi").ends_with("hi"));
        assert_eq!(Model::Granite.query_text("hi"), "hi");
    }

    #[test]
    fn thread_count_honours_the_override() {
        // Serialised against the other env-reading test by using a distinct value.
        unsafe { std::env::set_var(THREADS_ENV, "3") };
        assert_eq!(embed_threads(), 3);
        unsafe { std::env::remove_var(THREADS_ENV) };
        assert!(embed_threads() >= 1);
    }
}
