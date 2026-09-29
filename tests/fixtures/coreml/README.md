# Core ML known-answer fixture

- `chunks.json`: 512 real chunks sampled (seed 20260928) from the semlith store
  for the 2026-09-28 M1 Neural Engine bench.
- `ref_fp32.f32`: their embeddings, 512 x 384 little-endian f32, L2-normalised
  CLS of `ibm-granite/granite-embedding-small-english-r2` at revision
  `2ab6fa8ea2d674564defd37171ae19079b864b33` in fp32 (transformers), tokenised with
  the ONNX export's `tokenizer.json`, truncated at 400 tokens.

Regenerate the reference with `python packs/coreml/verify.py --write-ref --hf <snapshot>`;
it is byte-identical to the bench's `ref_fp32.npy`.
