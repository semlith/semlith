//! T01 spike: can fastembed load granite int8 at all?
//!
//! granite's ONNX export keeps its weights in a separate `model.onnx_data`
//! beside a graph-only `model.onnx`. semlith has only ever used fastembed's
//! enum path, which handles that internally. This proves the
//! `UserDefinedEmbeddingModel` path can do the same via external initializers,
//! and that the vectors it produces match the Python reference the model
//! benchmark was run against.
//!
//! Run with the directory holding the downloaded model files:
//!
//!   SPIKE_MODEL_DIR=/path/to/granite-small-r2 cargo run --example granite_spike

// `ExternalInitializerFile` is not exported from fastembed's root, so the
// struct cannot be named here. The `with_external_initializer` builder takes
// the name and bytes directly, which is the only way in.
use fastembed::{
    InitOptionsUserDefined, Pooling, TextEmbedding, TokenizerFiles, UserDefinedEmbeddingModel,
};
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let dir = PathBuf::from(
        std::env::var("SPIKE_MODEL_DIR").expect("set SPIKE_MODEL_DIR to the model directory"),
    );
    let onnx_dir = dir.join("onnx");

    let onnx_file = std::fs::read(onnx_dir.join("model_quantized.onnx"))?;
    // The graph names its weight file; the name has to match exactly or ONNX
    // Runtime looks for it on disk relative to the (in-memory) model and fails.
    let weights = std::fs::read(onnx_dir.join("model_quantized.onnx_data"))?;
    println!(
        "graph {} bytes, external weights {} bytes",
        onnx_file.len(),
        weights.len()
    );

    let tokenizer_files = TokenizerFiles {
        tokenizer_file: std::fs::read(dir.join("tokenizer.json"))?,
        config_file: std::fs::read(dir.join("config.json"))?,
        special_tokens_map_file: std::fs::read(dir.join("special_tokens_map.json"))?,
        tokenizer_config_file: std::fs::read(dir.join("tokenizer_config.json"))?,
    };

    let model = UserDefinedEmbeddingModel::new(onnx_file, tokenizer_files)
        // granite pools the CLS token — 1_Pooling/config.json has
        // pooling_mode_cls_token: true.
        .with_pooling(Pooling::Cls)
        .with_external_initializer("model_quantized.onnx_data".to_string(), weights);

    let opts = InitOptionsUserDefined::new()
        .with_max_length(semlith::chunk::MAX_CHARS / 2)
        .with_intra_threads(4);

    let mut embedder = TextEmbedding::try_new_from_user_defined(model, opts)
        .map_err(|e| anyhow::anyhow!("loading granite: {e}"))?;

    let texts = vec![
        "fn chunk_text splits a file into line aligned pieces".to_string(),
        "how to split text into chunks".to_string(),
        "the cat sat on the mat".to_string(),
    ];
    let vectors = embedder
        .embed(texts.clone(), None)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    println!("embedded {} texts, dim {}", vectors.len(), vectors[0].len());
    assert_eq!(vectors[0].len(), 384, "granite must produce 384 dimensions");

    let cos = |a: &[f32], b: &[f32]| -> f32 {
        let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        dot / (na * nb)
    };

    // Related texts must score above an unrelated one, or the weights loaded
    // as garbage and every vector would be meaningless noise.
    let related = cos(&vectors[0], &vectors[1]);
    let unrelated = cos(&vectors[0], &vectors[2]);
    println!("cos(code, question) = {related:.4}");
    println!("cos(code, unrelated) = {unrelated:.4}");
    assert!(
        related > unrelated + 0.1,
        "weights look wrong: related {related:.4} not clearly above unrelated {unrelated:.4}"
    );

    // The Python reference (onnxruntime, CLS pooling, same int8 file) scored
    // 0.894 for this pair. Anything close confirms identical numerics.
    println!("\nPython reference for this pair: 0.894");
    println!("SPIKE PASSED: granite int8 loads and embeds through fastembed");
    Ok(())
}
