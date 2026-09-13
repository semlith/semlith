//! Image support: what counts as an image, and the two CLIP models that make
//! one comparable with a sentence.
//!
//! A screenshots folder and a diagrams folder are exactly the content nobody
//! can grep, which is the gap this closes in "everything a person could read in
//! that folder". An image is embedded with CLIP ViT-B/32's vision encoder into
//! a second vector space, and a text query is embedded with the *matching* text
//! encoder into the same space — that pairing is the whole trick, and it is why
//! the two models here are fixed together rather than chosen.
//!
//! What this deliberately is not:
//!
//! - **OCR.** An image is matched by what it depicts, not by text read out of
//!   it. CLIP has no OCR to fall back on, so a dense text screenshot ranks
//!   poorly, and that is a limitation to state rather than to paper over.
//! - **A choice of model.** One pair, fixed, for the same reason the text model
//!   is fixed when a store is created: vectors from two models are not
//!   comparable and nothing downstream can tell.

use anyhow::{Context, Result};
use fastembed::{
    EmbeddingModel, ImageEmbedding, ImageEmbeddingModel, ImageInitOptions, TextEmbedding,
    TextInitOptions,
};
use std::path::Path;

/// The five extensions this reads. Deliberately short: no SVG (a document, not
/// a raster), no TIFF or HEIC (neither is what a developer's folder holds), no
/// PDF page rasterisation (a PDF is already read as text).
pub const EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "webp", "gif"];

/// The vector space images live in. CLIP ViT-B/32 on both sides.
pub const DIM: usize = 512;

/// What the About page and `semlith models` call it.
pub const MODEL_NAME: &str = "Qdrant/clip-ViT-B-32";

/// The two Hugging Face repositories it is actually two of. Named separately
/// because the airgap check asks whether *that* model is cached, and a machine
/// can hold one of them and not the other.
pub const VISION_REPO: &str = "Qdrant/clip-ViT-B-32-vision";
pub const TEXT_REPO: &str = "Qdrant/clip-ViT-B-32-text";

/// The commit each CLIP repository is pinned at, and the digest of every file
/// semlith will load out of it.
///
/// fastembed resolves these two as built-in models through its own client, so
/// semlith cannot hand it a revision the way `embed::load_granite` fetches
/// granite itself. What it can do — and does, before either encoder is used —
/// is refuse to load a cache whose bytes are not the ones this release was
/// built against. Recorded in `docs/models.md` with the commit URLs.
pub const VISION_REVISION: &str = "e0c24ed0fa57fa3e4f97f30de74c51d944036ace";
pub const TEXT_REVISION: &str = "48ca1db27cb4063eb311ec2aa7f087a808112876";

pub const VISION_FILES: &[(&str, &str)] = &[
    (
        "model.onnx",
        "c68d3d9a200ddd2a8c8a5510b576d4c94d1ae383bf8b36dd8c084f94e1fb4d63",
    ),
    (
        "config.json",
        "43bfed060ab82f57833bdd09acdcc2731995cb984651732a9d2b399e63113b9c",
    ),
    (
        "preprocessor_config.json",
        "ce945ef831c9972c135b5b198a03d8eeb70478cd69c0238f24caf1903a9965e6",
    ),
];

pub const TEXT_FILES: &[(&str, &str)] = &[
    (
        "model.onnx",
        "4dbe762b11e36488304471e439cde89da053ad7acaddbf9e096745d142ec8d8b",
    ),
    (
        "config.json",
        "4d5923d94bbc4e29864de837df14c138c12b93a0b738c64f3ca41f0c539e17b7",
    ),
    (
        "tokenizer.json",
        "b68d571997a1f81bf521fb73806740ddb91e4ed6666cb6e996c066bb289cf55b",
    ),
    (
        "special_tokens_map.json",
        "2cdb3b8331a60c92fc1e55a13e9fd61fd2293c5a51275fdcccd62b780052530e",
    ),
    (
        "tokenizer_config.json",
        "6bdcee9ccce2a16ca2b4c0c5ed00b42c50ea225f4472a8c4c1e963a2902c2881",
    ),
    (
        "vocab.json",
        "5047b556ce86ccaf6aa22b3ffccfc52d391ea4accdab9c2f2407da5b742d4363",
    ),
    (
        "merges.txt",
        "9fd691f7c8039210e0fced15865466c65820d09b63988b0174bfe25de299051a",
    ),
];

/// The directory the image vectors live in, beside the text index.
pub const INDEX_DIR: &str = "images";

/// Whether this path is one of the five formats, by extension.
///
/// By extension and not by content, for the same reason the text readers
/// dispatch that way: a store is searched far more often than it is built, and
/// sniffing every file's first bytes to decide costs the build pass nothing it
/// gets back.
pub fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|e| EXTENSIONS.contains(&e.as_str()))
}

/// An image's pixel size, read from its header rather than by decoding it.
pub fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

/// The two halves of CLIP, loaded on first use.
///
/// Both are fetched into the same model cache the text model uses, on the first
/// image a store indexes rather than at start — a store that never holds an
/// image never downloads them, which is the point. `--airgap` refuses the fetch
/// before a socket is opened, exactly as it does for the text model.
#[derive(Default)]
pub struct Clip {
    vision: Option<ImageEmbedding>,
    text: Option<TextEmbedding>,
}

impl Clip {
    /// Embed an image file. 512 floats, comparable with [`Clip::embed_query`].
    pub fn embed_image(&mut self, path: &Path, quiet: bool) -> Result<Vec<f32>> {
        let vision = match self.vision.as_mut() {
            Some(model) => model,
            None => {
                crate::embed::refuse_if_airgapped(VISION_REPO)?;
                let cache = crate::model_cache_dir();
                crate::embed::verify_cached(&cache, VISION_REPO, VISION_REVISION, VISION_FILES)?;
                let options = ImageInitOptions::new(ImageEmbeddingModel::ClipVitB32)
                    .with_show_download_progress(!quiet)
                    .with_cache_dir(cache.clone());
                let model = ImageEmbedding::try_new(options)
                    .map_err(|e| anyhow::anyhow!("loading {MODEL_NAME}'s vision encoder: {e}"))?;
                // Again, now that fastembed has fetched: the check above covers
                // a cache that was already there, and this one covers the
                // download this call just made.
                crate::embed::verify_cached(&cache, VISION_REPO, VISION_REVISION, VISION_FILES)?;
                self.vision.insert(model)
            }
        };
        let mut out = vision
            .embed(vec![path], None)
            .with_context(|| format!("embedding {}", path.display()))?;
        Ok(out.remove(0))
    }

    /// Embed a text query into the *image* space.
    ///
    /// The matching text encoder, not the store's own model: a granite vector
    /// and a CLIP vector are numbers of different lengths about different
    /// things, and comparing them would return whatever the arithmetic
    /// happened to produce.
    pub fn embed_query(&mut self, query: &str, quiet: bool) -> Result<Vec<f32>> {
        let text = match self.text.as_mut() {
            Some(model) => model,
            None => {
                crate::embed::refuse_if_airgapped(TEXT_REPO)?;
                let cache = crate::model_cache_dir();
                crate::embed::verify_cached(&cache, TEXT_REPO, TEXT_REVISION, TEXT_FILES)?;
                let options = TextInitOptions::new(EmbeddingModel::ClipVitB32)
                    .with_show_download_progress(!quiet)
                    .with_intra_threads(crate::embed::embed_threads())
                    .with_cache_dir(cache.clone());
                let model = TextEmbedding::try_new(options)
                    .map_err(|e| anyhow::anyhow!("loading {MODEL_NAME}'s text encoder: {e}"))?;
                crate::embed::verify_cached(&cache, TEXT_REPO, TEXT_REVISION, TEXT_FILES)?;
                self.text.insert(model)
            }
        };
        let mut out = text
            .embed(vec![query], None)
            .map_err(|e| anyhow::anyhow!("embedding the query for the image index: {e}"))?;
        Ok(out.remove(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_five_formats_are_recognised_by_extension_in_any_case() {
        for name in ["a.png", "b.JPG", "c.jpeg", "d.webp", "e.GIF"] {
            assert!(is_image(Path::new(name)), "{name} is not read as an image");
        }
        for name in ["a.svg", "b.tiff", "c.heic", "d.pdf", "e.rs", "noextension"] {
            assert!(!is_image(Path::new(name)), "{name} is read as an image");
        }
    }

    /// The header is enough: nothing decodes a whole image to find its size.
    #[test]
    fn dimensions_come_from_the_header() {
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::new(2, 3))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        assert_eq!(dimensions(&png), Some((2, 3)));
        assert_eq!(dimensions(b"not an image at all"), None);
    }
}
