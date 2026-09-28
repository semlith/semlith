//! Embedding from token ids, which the index pass produces once per chunk.
//!
//! fastembed tokenises inside `embed`, so before 0.32.0 a chunk was tokenised
//! to sort its window by length, again by fastembed on the CPU lane, and a
//! third time inside a GPU worker. A run now tokenises each chunk once, in the
//! prepare stage, and everything downstream — the CPU session here, every
//! worker lane — is handed ids. Queries still go through fastembed: the query
//! path is not this release's to change.

use anyhow::{Context, Result};
use std::path::Path;

/// One chunk as the model sees it: its token ids, the special tokens included
/// and the truncation applied, without padding.
pub type Ids = Vec<u32>;

/// granite's vector width.
pub const DIM: usize = 384;

/// The id granite pads with. The attention mask is what keeps it out of the
/// answer, so its value only has to be a valid token.
pub const PAD_ID: u32 = 0;

/// granite's tokenizer set up for the prepare stage: the same truncation and
/// special tokens the CPU session always had, and no padding, because a chunk
/// is encoded alone and padded later, to the batch it lands in.
pub fn tokenizer(cache: &Path) -> Result<tokenizers::Tokenizer> {
    let mut tokenizer = crate::embed::granite_tokenizer(cache, crate::chunk::MAX_CHARS / 2)?;
    tokenizer.with_padding(None);
    Ok(tokenizer)
}

/// Encode one chunk's embedded text.
pub fn encode(tokenizer: &tokenizers::Tokenizer, text: &str) -> Result<Ids> {
    Ok(tokenizer
        .encode(text, true)
        .map_err(|e| anyhow::anyhow!("tokenizing: {e}"))?
        .get_ids()
        .to_vec())
}

/// A batch padded to its longest member: ids and mask, row-major, as `i64`
/// because that is what granite's ONNX graph takes.
pub fn padded(batch: &[&[u32]]) -> (usize, usize, Vec<i64>, Vec<i64>) {
    let rows = batch.len();
    let seq = batch.iter().map(|ids| ids.len()).max().unwrap_or(0).max(1);
    let mut ids = vec![PAD_ID as i64; rows * seq];
    let mut mask = vec![0i64; rows * seq];
    for (row, chunk) in batch.iter().enumerate() {
        for (at, id) in chunk.iter().enumerate() {
            ids[row * seq + at] = *id as i64;
            mask[row * seq + at] = 1;
        }
    }
    (rows, seq, ids, mask)
}

/// Run a granite ONNX session over one batch of ids and return each row's
/// CLS vector, normalised — the pooling granite's own config names.
pub fn run(session: &mut ort::session::Session, batch: &[&[u32]]) -> Result<Vec<Vec<f32>>> {
    if batch.is_empty() {
        return Ok(Vec::new());
    }
    let (rows, seq, ids, mask) = padded(batch);
    let outputs = session
        .run(ort::inputs![
            "input_ids" => ort::value::Tensor::from_array(([rows, seq], ids)).map_err(|e| anyhow::anyhow!("{e}"))?,
            "attention_mask" => ort::value::Tensor::from_array(([rows, seq], mask)).map_err(|e| anyhow::anyhow!("{e}"))?,
        ])
        .map_err(|e| anyhow::anyhow!("running the model: {e}"))?;
    let (shape, data) = outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let dim = *shape.last().context("an output with no shape")? as usize;
    let mut vectors = Vec::with_capacity(rows);
    for row in 0..rows {
        let start = row * seq * dim;
        let mut vector = data[start..start + dim].to_vec();
        crate::normalize(&mut vector);
        vectors.push(vector);
    }
    Ok(vectors)
}

/// The index pass's CPU session: one of granite's pinned variants on ONNX
/// Runtime's CPU provider, fed ids.
pub struct CpuSession {
    session: ort::session::Session,
    variant: crate::embed::Variant,
    threads: usize,
}

impl CpuSession {
    /// Spinning is off on both pools. ORT's threads otherwise spin between
    /// operators for the whole length of a run, and on Apple silicon that is
    /// CPU taken from the Neural Engine lane's feeders and the prepare stage
    /// for no work (measured 30.7 → 35.5 chunks/s with it off, 8 threads).
    pub fn open(
        cache: &Path,
        variant: crate::embed::Variant,
        threads: usize,
        quiet: bool,
    ) -> Result<Self> {
        crate::embed::link_runtime()?;
        let graph = crate::embed::granite_graph(cache, quiet, variant)?;
        let threads = threads.max(1);
        let session = ort::session::Session::builder()
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level3)
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .with_intra_threads(threads)
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .with_intra_op_spinning(false)
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .with_inter_op_spinning(false)
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .commit_from_file(&graph)
            .map_err(|e| anyhow::anyhow!("loading {}: {e}", graph.display()))?;
        Ok(Self {
            session,
            variant,
            threads,
        })
    }

    pub fn embed(&mut self, batch: &[&[u32]]) -> Result<Vec<Vec<f32>>> {
        run(&mut self.session, batch)
    }

    pub fn variant(&self) -> crate::embed::Variant {
        self.variant
    }

    pub fn threads(&self) -> usize {
        self.threads
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_batch_is_padded_to_its_longest_row_and_masked() {
        let (rows, seq, ids, mask) = padded(&[&[5, 6, 7], &[8]]);
        assert_eq!((rows, seq), (2, 3));
        assert_eq!(ids, vec![5, 6, 7, 8, 0, 0]);
        assert_eq!(mask, vec![1, 1, 1, 1, 0, 0]);
    }
}
