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
                let options = ImageInitOptions::new(ImageEmbeddingModel::ClipVitB32)
                    .with_show_download_progress(!quiet)
                    .with_cache_dir(crate::model_cache_dir());
                let model = ImageEmbedding::try_new(options)
                    .map_err(|e| anyhow::anyhow!("loading {MODEL_NAME}'s vision encoder: {e}"))?;
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
                let options = TextInitOptions::new(EmbeddingModel::ClipVitB32)
                    .with_show_download_progress(!quiet)
                    .with_intra_threads(crate::embed::embed_threads())
                    .with_cache_dir(crate::model_cache_dir());
                let model = TextEmbedding::try_new(options)
                    .map_err(|e| anyhow::anyhow!("loading {MODEL_NAME}'s text encoder: {e}"))?;
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
