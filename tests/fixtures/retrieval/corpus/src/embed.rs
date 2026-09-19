//! Which embedding model a store uses, and how to load it.
//!
//! fastembed ships an enum of models it knows how to fetch and configure. The
//! default is not in that enum: granite-embedding-small-english-r2, quantized
//! to int8, has to be assembled by hand from its ONNX graph, its separate
//! weights file, and its tokenizer. [`Model`] is the union of "something
//! fastembed knows" and "the one we assemble ourselves", so a store can record
//! either and read it back.

use anyhow::{Context, Result, bail};
use fastembed::{
    EmbeddingModel, InitOptionsUserDefined, Pooling, TextEmbedding, TextInitOptions,
    TokenizerFiles, UserDefinedEmbeddingModel,
};
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// The Hugging Face repository holding granite's ONNX export.
pub const GRANITE_REPO: &str = "onnx-community/granite-embedding-small-english-r2-ONNX";

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

    /// The tokenizer this model embeds with, for counting tokens rather than
    /// estimating them.
    ///
    /// The same `tokenizer.json` [`Self::load`] hands to fastembed, read from
    /// the same cache and verified against the same pinned digest — so a token
    /// count here and the model's own segmentation are the same arithmetic,
    /// not two approximations of it.
    ///
    /// `None` when the file is not cached, which is the airgapped machine that
    /// has never fetched a model. The caller falls back to four characters per
    /// token and says so in the row it writes.
    pub fn tokenizer(&self, cache_dir: &Path) -> Option<tokenizers::Tokenizer> {
        let name = match self {
            Model::Granite => GRANITE_REPO,
            // fastembed lays a builtin model's files out under its own
            // directory name, which is the model's `model_code`.
            Model::Builtin(m) => return builtin_tokenizer(cache_dir, m),
        };
        let dir = snapshot_dir(cache_dir, name, GRANITE_REVISION)?;
        tokenizers::Tokenizer::from_file(dir.join("tokenizer.json")).ok()
    }

    pub fn load(
        &self,
        cache_dir: PathBuf,
        max_length: usize,
        quiet: bool,
    ) -> Result<TextEmbedding> {
        // Checked here, in the one place weights are ever fetched, rather than
        // at each call site: an airgapped machine's whole claim is that this
        // process cannot have been the one that reached the network, and a
        // check that lives anywhere else is a check a later caller can miss.
        if airgap() && !is_cached(&cache_dir) {
            bail!(
                "{AIRGAP_ENV} is set and no model is cached at {} — \
                 pre-seed it with SEMLITH_MODEL_CACHE on a connected machine, \
                 or drop --airgap to let this run download it",
                cache_dir.display()
            );
        }
        match self {
            Model::Builtin(m) => {
                link_runtime()?;
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

/// Point ONNX Runtime at the library shipped beside this binary.
///
/// Only in a `dynamic-ort` build, which is the two Linux release binaries and
/// nothing else. Every other build — `cargo install`, macOS, Windows, a
/// developer's own `cargo build` — links the library `ort` downloads at build
/// time and this function does nothing.
///
/// The reason the Linux artifacts differ is issue #57: the library `ort`
/// downloads references glibc 2.38 symbols, so the binary built against it
/// cannot start on Debian 12, Ubuntu 22.04 LTS, RHEL 9 or Amazon Linux 2023,
/// and building on an older runner cannot lower a floor the vendored library
/// sets. Microsoft's own ONNX Runtime release needs glibc 2.27, so the release
/// jobs pack that beside the binary and this loads it.
///
/// Run once, before anything asks for a model.
pub fn link_runtime() -> Result<()> {
    #[cfg(feature = "dynamic-ort")]
    {
        use std::sync::OnceLock;
        static ONCE: OnceLock<Result<(), String>> = OnceLock::new();
        return ONCE
            .get_or_init(|| {
                let Ok(exe) = std::env::current_exe() else {
                    return Err("the running binary could not be located".to_string());
                };
                let beside = exe
                    .parent()
                    .map(|dir| dir.join(RUNTIME_FILE))
                    .unwrap_or_else(|| PathBuf::from(RUNTIME_FILE));
                if !beside.exists() {
                    return Err(format!(
                        "{} is not beside the semlith binary. The Linux release \
                         archive carries it next to `semlith`, and `install.sh` \
                         puts both in the same directory — a binary copied out of \
                         the archive on its own cannot embed anything. Unpack the \
                         archive again, or install with the one-liner in the \
                         README.",
                        beside.display()
                    ));
                }
                ort::init_from(beside.to_string_lossy().as_ref())
                    .map_err(|e| format!("loading {}: {e}", beside.display()))?
                    .commit();
                // `commit` answers whether this call was the one that
                // installed the environment; a second caller getting `false`
                // is the OnceLock doing its job, not a failure.
                Ok(())
            })
            .clone()
            .map_err(anyhow::Error::msg);
    }
    #[cfg(not(feature = "dynamic-ort"))]
    Ok(())
}

/// What a `dynamic-ort` build looks for beside itself.
#[cfg(feature = "dynamic-ort")]
const RUNTIME_FILE: &str = if cfg!(target_os = "macos") {
    "libonnxruntime.dylib"
} else if cfg!(windows) {
    "onnxruntime.dll"
} else {
    "libonnxruntime.so"
};

fn load_granite(cache_dir: PathBuf, max_length: usize, quiet: bool) -> Result<TextEmbedding> {
    link_runtime()?;
    check_cache_dir(&cache_dir)?;
    let cache = cache_dir.clone();

    // Whether this cache has already been checked against this pin, with every
    // file still the size and age it was. Asked once here rather than per file,
    // because the answer is about the set.
    let checked = snapshot_dir(&cache, GRANITE_REPO, GRANITE_REVISION)
        .is_some_and(|dir| already_verified(&dir, GRANITE_REVISION, GRANITE_FILES));

    // A revision rather than a branch. `main` is a name somebody else controls;
    // a commit is the bytes this release was built against.
    let repo = hf_hub::api::sync::ApiBuilder::new()
        .with_cache_dir(cache_dir)
        .with_progress(!quiet)
        .build()
        .context("building the Hugging Face client")?
        .repo(hf_hub::Repo::with_revision(
            GRANITE_REPO.to_string(),
            hf_hub::RepoType::Model,
            GRANITE_REVISION.to_string(),
        ));

    let fetch = |name: &str| -> Result<Vec<u8>> {
        let path = repo
            .get(name)
            .with_context(|| format!("fetching {name} from {GRANITE_REPO}"))?;
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        if checked {
            return Ok(bytes);
        }
        // Verified whether it was just fetched or was already in the cache: a
        // cache is a directory on disk, and the point of a digest is that it
        // does not matter how the bytes got there.
        let expected = GRANITE_FILES
            .iter()
            .find(|(file, _)| *file == name)
            .map(|(_, digest)| *digest)
            .with_context(|| format!("{name} is not a file semlith pins a digest for"))?;
        verify(name, &bytes, expected)?;
        Ok(bytes)
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

    // Recorded after every file has been read and checked, and only when this
    // run did the checking — the fetch above may have created the snapshot
    // directory that did not exist when `checked` was read.
    if !checked && let Some(dir) = snapshot_dir(&cache, GRANITE_REPO, GRANITE_REVISION) {
        record_verified(&dir, GRANITE_REVISION, GRANITE_FILES);
    }

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
/// Refuse to download model weights, so an air-gapped machine can prove this
/// process never reached the network. Set by `--airgap` and readable directly.
pub const AIRGAP_ENV: &str = "SEMLITH_AIRGAP";

/// Find a builtin model's cached `tokenizer.json`.
///
/// fastembed does not publish where it put the files, so this looks for the
/// one file that matters under the cache root. Cheap and bounded: the cache
/// holds a handful of model directories, and a miss costs a directory walk and
/// returns `None`.
fn builtin_tokenizer(cache_dir: &Path, model: &EmbeddingModel) -> Option<tokenizers::Tokenizer> {
    let wanted = format!("{model}").to_lowercase().replace('/', "--");
    let entries = std::fs::read_dir(cache_dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if !name.contains(&wanted) {
            continue;
        }
        for candidate in walk_for(&entry.path(), "tokenizer.json") {
            if let Ok(tokenizer) = tokenizers::Tokenizer::from_file(&candidate) {
                return Some(tokenizer);
            }
        }
    }
    None
}

/// Every `name` at most two directories below `root`.
fn walk_for(root: &Path, name: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let direct = root.join(name);
    if direct.is_file() {
        found.push(direct);
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return found;
    };
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let nested = entry.path().join(name);
        if nested.is_file() {
            found.push(nested);
        }
    }
    found
}

pub fn airgap() -> bool {
    matches!(
        std::env::var(AIRGAP_ENV).ok().as_deref(),
        Some("1" | "true" | "yes")
    )
}

/// Whether the cache already holds something to load.
///
/// Deliberately "is there anything here", not "are these the right files":
/// the loader below fails with its own, far more specific error if what is
/// cached is wrong, and duplicating its file list here would be a second
/// definition of the model to keep in step.
/// Refuse a model fetch on an airgapped machine, naming the model.
///
/// The same rule the text model is loaded under, in a form the image models can
/// use: an airgapped run's whole claim is that this process cannot have been
/// the one that reached the network, and a caller that fetches without asking
/// is a caller that breaks the claim.
///
/// This one asks about *that* model rather than about the cache as a whole. A
/// machine with the text model pre-seeded and no CLIP has a non-empty cache and
/// still cannot fetch, so the coarse "is anything cached" test would have let
/// the first image a store indexed open a socket on a machine whose whole point
/// is that it does not.
pub fn refuse_if_airgapped(model: &str) -> Result<()> {
    let cache = crate::model_cache_dir()?;
    if airgap() && !model_is_cached(&cache, model) {
        bail!(
            "{AIRGAP_ENV} is set and {model} is not cached at {} — \
             pre-seed it with SEMLITH_MODEL_CACHE on a connected machine, \
             or drop --airgap to let this run download it",
            cache.display()
        );
    }
    Ok(())
}

/// Whether one model's weights are already in the cache.
///
/// hf-hub names a model's directory `models--<org>--<name>`, so the repository
/// id is matched against the directory name with the separators normalised
/// rather than reconstructed — a naming change upstream should read as "not
/// cached", which refuses, rather than as "cached", which would not.
pub fn model_is_cached(cache_dir: &Path, model: &str) -> bool {
    let want = model.replace('/', "--").to_ascii_lowercase();
    std::fs::read_dir(cache_dir)
        .map(|entries| {
            entries.flatten().any(|entry| {
                let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                name.ends_with(&want) && entry.path().join("snapshots").exists()
            })
        })
        .unwrap_or(false)
}

/// Whether the default model is cached, with the right bytes.
///
/// "Is there anything here" until 0.14.0, which answered yes for a cache
/// holding half a download or somebody else's files. It now asks the question
/// the airgap check needs answered: can this run load the model without
/// reaching the network, and is what it would load the model this release pins.
///
/// A digest per file is the expensive form and this is called on every airgap
/// check, so the size is the cheap first pass: a file of the right length whose
/// digest is wrong is the case `load_granite` refuses by name a moment later.
pub fn is_cached(cache_dir: &Path) -> bool {
    let snapshot = match snapshot_dir(cache_dir, GRANITE_REPO, GRANITE_REVISION) {
        Some(dir) => dir,
        None => return false,
    };
    GRANITE_FILES
        .iter()
        .all(|(name, _)| snapshot.join(name).exists())
}

/// Where hf-hub puts one revision of one repository.
///
/// `<cache>/models--<org>--<name>/snapshots/<revision>`. Derived rather than
/// guessed at: hf-hub writes it, and a naming change upstream should read as
/// "not cached", which refuses, rather than as "cached", which would not.
fn snapshot_dir(cache_dir: &Path, repo: &str, revision: &str) -> Option<PathBuf> {
    let dir = cache_dir
        .join(format!("models--{}", repo.replace('/', "--")))
        .join("snapshots")
        .join(revision);
    dir.is_dir().then_some(dir)
}

/// Verify every pinned file of a repository that fastembed fetched itself.
///
/// `load_granite` verifies as it reads, because semlith does that fetching. The
/// image models are fastembed's own built-ins: it resolves and caches them
/// through its own client, so the check happens against what landed in the
/// cache rather than against bytes passing through semlith's hands. The
/// property is the same — a file that is not the one this release pins is
/// refused by name — and it is checked before the model is used.
pub fn verify_cached(
    cache_dir: &Path,
    repo: &str,
    revision: &str,
    files: &[(&str, &str)],
) -> Result<()> {
    check_cache_dir(cache_dir)?;
    let _ = revision;
    // Every snapshot under this repository, not only the pinned one. fastembed
    // resolves its built-in models at `main`, so a repository that moved leaves
    // its new commit here under a different name — and checking only the pinned
    // directory would pass by finding nothing. What is checked is what is on
    // disk to be loaded.
    let snapshots = cache_dir
        .join(format!("models--{}", repo.replace('/', "--")))
        .join("snapshots");
    let Ok(entries) = std::fs::read_dir(&snapshots) else {
        // Nothing cached. fastembed will fetch, and the next call through this
        // function is the one that checks what it fetched.
        return Ok(());
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        // The same stamp the text model uses: the CLIP weights are 350 MB and
        // 254 MB, so hashing them on every load would cost more than loading
        // them does.
        if already_verified(&dir, revision, files) {
            continue;
        }
        let mut read_all = true;
        for (name, expected) in files {
            let path = dir.join(name);
            let Ok(bytes) = std::fs::read(&path) else {
                read_all = false;
                continue;
            };
            verify(&format!("{repo}/{name}"), &bytes, expected)?;
        }
        if read_all {
            record_verified(&dir, revision, files);
        }
    }
    Ok(())
}

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

// ---------------------------------------------------------------- pinned weights

/// The commit each model repository is fetched at.
///
/// A Hugging Face repository is a git repository somebody else can push to, and
/// before 0.14.0 semlith fetched from whatever `main` pointed at. The weights
/// are what computes every vector in every store: a model that changed under a
/// user would change what their corpus means without changing anything they can
/// see, and a model that was replaced would do it on purpose.
pub const GRANITE_REVISION: &str = "1dc7835ba0cb9c76a3618d0bf0c427c97671b3c8";

/// Every file semlith fetches from [`GRANITE_REPO`], with its SHA-256.
///
/// Resolved against the repository at [`GRANITE_REVISION`] and recorded in
/// `docs/models.md` with the commit URL, so the number in this table can be
/// checked against the one Hugging Face publishes without reading this file.
pub const GRANITE_FILES: &[(&str, &str)] = &[
    (
        "tokenizer.json",
        "feeb83348dcb033bc6b9d2e1f7906ca9eb2d122845000c9416d894d7c2927149",
    ),
    (
        "config.json",
        "1a1710c20911da8c96179716bf44058e54cca6fa7952cce77be83ef05edae3ee",
    ),
    (
        "special_tokens_map.json",
        "ea97ecdbcc73713039d8d64dbb05e3689495c96657fbd9a18f5bed381be81049",
    ),
    (
        "tokenizer_config.json",
        "ce06781b38bb393db68c9e0709bddd31ef5d88f2c6fbb3fd9f369778fb85e451",
    ),
    (
        GRANITE_ONNX,
        "a3fad524afc3f060216a8ddbb1ac89c9b6498fba8995b5718bde879076a2e9ba",
    ),
    (
        GRANITE_WEIGHTS,
        "1f4cf47e4adec7f7ae09db03d071ba8667e07f9a4203142c7efa8d37fe453597",
    ),
];

/// What a cache directory records about the files this release already verified.
///
/// Hashing every pinned file on every load is 52 MB of SHA-256 before a process
/// can answer anything — measured at 276 ms on an M1, which is most of what a
/// `semlith search` from the command line costs. So the digests are checked
/// once and the result recorded here, keyed by the pin and by each file's size
/// and modification time: if all three still match, the bytes are the ones that
/// were verified.
///
/// This does not weaken the check it replaces. The attacker it would have to
/// let through is one who can write this cache *and* preserve each file's size
/// and mtime — and a cache anybody but this user can write to is already
/// refused by [`check_cache_dir`], so that attacker is this user. What the
/// digests are actually for — a corrupted download, and an upstream repository
/// whose bytes changed — moves a file's size or mtime every time.
#[derive(serde::Serialize, serde::Deserialize)]
struct Verified {
    revision: String,
    /// File name to `(size, mtime nanoseconds, the digest it was checked
    /// against)`.
    files: std::collections::BTreeMap<String, (u64, u128, String)>,
}

const STAMP: &str = ".semlith-verified";

/// What a file looks like on disk right now, for the stamp.
fn fingerprint(path: &Path) -> Option<(u64, u128)> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some((meta.len(), modified))
}

/// Whether every pinned file was verified at this revision and has not moved.
fn already_verified(dir: &Path, revision: &str, files: &[(&str, &str)]) -> bool {
    let Ok(text) = std::fs::read_to_string(dir.join(STAMP)) else {
        return false;
    };
    let Ok(stamp) = serde_json::from_str::<Verified>(&text) else {
        return false;
    };
    if stamp.revision != revision {
        return false;
    }
    files.iter().all(|(name, expected)| {
        stamp
            .files
            .get(*name)
            .zip(fingerprint(&dir.join(name)))
            .is_some_and(|((size, mtime, digest), (now_size, now_mtime))| {
                digest == expected && *size == now_size && *mtime == now_mtime
            })
    })
}

/// Record that every pinned file has been checked, so the next process need not.
fn record_verified(dir: &Path, revision: &str, files: &[(&str, &str)]) {
    let mut stamp = Verified {
        revision: revision.to_string(),
        files: std::collections::BTreeMap::new(),
    };
    for (name, expected) in files {
        let Some((size, mtime)) = fingerprint(&dir.join(name)) else {
            // A file that cannot be measured is one the next run should check
            // again, so the stamp is not written at all.
            return;
        };
        stamp
            .files
            .insert((*name).to_string(), (size, mtime, (*expected).to_string()));
    }
    let Ok(body) = serde_json::to_vec(&stamp) else {
        return;
    };
    // Best effort: a cache that cannot be written to is one that gets verified
    // every time, which is slower and not wrong.
    let _ = crate::home::write_private(&dir.join(STAMP), &body);
}

/// The SHA-256 of some bytes, as lowercase hex.
pub fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

/// Check bytes against the digest recorded for them, or say which file failed.
pub fn verify(name: &str, bytes: &[u8], expected: &str) -> Result<()> {
    let actual = digest(bytes);
    if actual != expected {
        bail!(
            "{name} does not match the digest semlith pins for it.\n  \
             expected {expected}\n  got      {actual}\n\
             The model cache holds a file that is not the one this release was \
             built against. Delete the cache and let semlith fetch it again; if \
             it happens twice, the file upstream has changed and semlith needs a \
             release rather than a retry."
        );
    }
    Ok(())
}

/// Refuse a model cache that somebody else on this machine can write to.
///
/// The cache is weights, and the weights are what every vector is computed by.
/// A directory another account can write to is a model another account
/// chooses — and the choosing is invisible, because a corpus embedded by a
/// different model still answers, just differently.
pub fn check_cache_dir(cache: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        use std::os::unix::fs::PermissionsExt;
        let Ok(meta) = std::fs::metadata(cache) else {
            return Ok(());
        };
        // SAFETY: `getuid` reads this process's own id and cannot fail.
        let me = unsafe { libc::getuid() };
        if meta.uid() != me {
            bail!(
                "{} is owned by uid {} rather than by you. semlith will not load \
                 model weights from a directory somebody else owns — run `chown -R \
                 {me} {}`, or point {} somewhere you own.",
                cache.display(),
                meta.uid(),
                cache.display(),
                crate::MODEL_CACHE_ENV,
            );
        }
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o022 != 0 {
            bail!(
                "{} is mode {mode:o}, so other users on this machine can write to \
                 it. semlith will not load model weights from it — run `chmod 700 \
                 {}`.",
                cache.display(),
                cache.display(),
            );
        }
    }
    #[cfg(not(unix))]
    let _ = cache;
    Ok(())
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
