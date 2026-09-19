# The models semlith loads, and the bytes it will accept

semlith computes every vector in every store with one of three ONNX models. They
are fetched from Hugging Face on first use, cached under `~/.cache/semlith/models`
(or wherever `SEMLITH_MODEL_CACHE` names), and never fetched again.

From 0.14.0 each one is pinned to a commit and each file is verified against a
SHA-256 recorded in the source beside the repository name. A Hugging Face
repository is a git repository somebody else can push to, and the weights are
what decides what a corpus means: a model that changed under you would change
every answer without changing anything you can see, and a model that was
replaced would do it deliberately.

A file whose digest does not match is refused by name, with both digests
printed, and nothing is loaded. That is a release-level event rather than
something to retry: if it happens twice on a clean cache, the bytes upstream
have changed and semlith needs a new release rather than another download.

## Text: granite-embedding-small-english-r2

The default model, 384 dimensions, int8, Apache-2.0. semlith fetches these files
itself, so each one is verified as it is read rather than after the fact.

- Repository: [`onnx-community/granite-embedding-small-english-r2-ONNX`](https://huggingface.co/onnx-community/granite-embedding-small-english-r2-ONNX)
- Commit: [`1dc7835ba0cb9c76a3618d0bf0c427c97671b3c8`](https://huggingface.co/onnx-community/granite-embedding-small-english-r2-ONNX/tree/1dc7835ba0cb9c76a3618d0bf0c427c97671b3c8)
- Recorded in `src/embed.rs` as `GRANITE_REVISION` and `GRANITE_FILES`.

| File | SHA-256 |
| --- | --- |
| `tokenizer.json` | `feeb83348dcb033bc6b9d2e1f7906ca9eb2d122845000c9416d894d7c2927149` |
| `config.json` | `1a1710c20911da8c96179716bf44058e54cca6fa7952cce77be83ef05edae3ee` |
| `special_tokens_map.json` | `ea97ecdbcc73713039d8d64dbb05e3689495c96657fbd9a18f5bed381be81049` |
| `tokenizer_config.json` | `ce06781b38bb393db68c9e0709bddd31ef5d88f2c6fbb3fd9f369778fb85e451` |
| `onnx/model_quantized.onnx` | `a3fad524afc3f060216a8ddbb1ac89c9b6498fba8995b5718bde879076a2e9ba` |
| `onnx/model_quantized.onnx_data` | `1f4cf47e4adec7f7ae09db03d071ba8667e07f9a4203142c7efa8d37fe453597` |

## Rescoring: jina-reranker-v1-turbo-en

A cross-encoder, 37 M parameters, int8, Apache-2.0. It reads the query and a
candidate together and reorders the head of the fused list; it embeds nothing
and adds no candidates, so a store's vectors do not depend on it. Searches run
without it — by fusion alone — when it is not in the cache, and `semlith stats`
and `semlith doctor` both say which of the two is happening.

`semlith setup` fetches it beside the embedding model so that a query never
pauses to download one. `SEMLITH_RERANK=off` switches the stage off for a
process, which is how its contribution is measured: the same binary and the
same store, once each way.

- Repository: [`jinaai/jina-reranker-v1-turbo-en`](https://huggingface.co/jinaai/jina-reranker-v1-turbo-en)
- Commit: [`b8c14f4e723d9e0aab4732a7b7b93741eeeb77c2`](https://huggingface.co/jinaai/jina-reranker-v1-turbo-en/tree/b8c14f4e723d9e0aab4732a7b7b93741eeeb77c2)
- Recorded in `src/rerank.rs` as `RERANK_REVISION` and `RERANK_FILES`.

| File | SHA-256 |
| --- | --- |
| `tokenizer.json` | `0046da43cc8c424b317f56b092b0512aaaa65c4f925d2f16af9d9eeb4d0ef902` |
| `config.json` | `e050ff6a15ae9295e84882fa0e98051bd8754856cd5201395ebf00ce9f2d609b` |
| `special_tokens_map.json` | `06e405a36dfe4b9604f484f6a1e619af1a7f7d09e34a8555eb0b77b66318067f` |
| `tokenizer_config.json` | `d291c6652d96d56ffdbcf1ea19d9bae5ed79003f7648c627e725a619227ce8fa` |
| `onnx/model_quantized.onnx` | `3defdef1ae34e119bd704216087743e79665934c96aebabcb6077c239dc3ae66` |

## Images: CLIP ViT-B/32

Two repositories, a vision encoder and a text encoder, fixed as a pair: a query
about a picture goes through CLIP's own text encoder, never through the store's
model, because a granite vector and a CLIP vector are numbers of different
lengths about different things.

These are fastembed's built-in models, so fastembed resolves and caches them
through its own client and semlith cannot hand it a revision. What semlith does
instead is check the cache — every snapshot in it, not only the pinned one —
before either encoder is used and again after a fetch, and refuse to load bytes
that are not the ones this release was built against.

- Vision: [`Qdrant/clip-ViT-B-32-vision`](https://huggingface.co/Qdrant/clip-ViT-B-32-vision) at
  [`e0c24ed0fa57fa3e4f97f30de74c51d944036ace`](https://huggingface.co/Qdrant/clip-ViT-B-32-vision/tree/e0c24ed0fa57fa3e4f97f30de74c51d944036ace)
- Text: [`Qdrant/clip-ViT-B-32-text`](https://huggingface.co/Qdrant/clip-ViT-B-32-text) at
  [`48ca1db27cb4063eb311ec2aa7f087a808112876`](https://huggingface.co/Qdrant/clip-ViT-B-32-text/tree/48ca1db27cb4063eb311ec2aa7f087a808112876)
- Recorded in `src/image.rs` as `VISION_REVISION`, `VISION_FILES`, `TEXT_REVISION` and `TEXT_FILES`.

| Repository | File | SHA-256 |
| --- | --- | --- |
| vision | `model.onnx` | `c68d3d9a200ddd2a8c8a5510b576d4c94d1ae383bf8b36dd8c084f94e1fb4d63` |
| vision | `config.json` | `43bfed060ab82f57833bdd09acdcc2731995cb984651732a9d2b399e63113b9c` |
| vision | `preprocessor_config.json` | `ce945ef831c9972c135b5b198a03d8eeb70478cd69c0238f24caf1903a9965e6` |
| text | `model.onnx` | `4dbe762b11e36488304471e439cde89da053ad7acaddbf9e096745d142ec8d8b` |
| text | `config.json` | `4d5923d94bbc4e29864de837df14c138c12b93a0b738c64f3ca41f0c539e17b7` |
| text | `tokenizer.json` | `b68d571997a1f81bf521fb73806740ddb91e4ed6666cb6e996c066bb289cf55b` |
| text | `special_tokens_map.json` | `2cdb3b8331a60c92fc1e55a13e9fd61fd2293c5a51275fdcccd62b780052530e` |
| text | `tokenizer_config.json` | `6bdcee9ccce2a16ca2b4c0c5ed00b42c50ea225f4472a8c4c1e963a2902c2881` |
| text | `vocab.json` | `5047b556ce86ccaf6aa22b3ffccfc52d391ea4accdab9c2f2407da5b742d4363` |
| text | `merges.txt` | `9fd691f7c8039210e0fced15865466c65820d09b63988b0174bfe25de299051a` |

## The cache directory itself

semlith refuses to load weights from a model cache that is owned by another
account or that other users on the machine can write to, naming the `chown` or
`chmod` that fixes it. A directory somebody else can write to is a model
somebody else chooses, and a corpus embedded by a different model still answers —
just differently, which is the hard kind of wrong to notice.

## Checking a digest yourself

Nothing here has to be taken on trust. Hugging Face publishes the SHA-256 of
every LFS file in its tree API, and the small ones can be hashed directly:

```sh
curl -sSL "https://huggingface.co/onnx-community/granite-embedding-small-english-r2-ONNX/resolve/1dc7835ba0cb9c76a3618d0bf0c427c97671b3c8/tokenizer.json" \
  | shasum -a 256
```

```sh
curl -sS "https://huggingface.co/api/models/Qdrant/clip-ViT-B-32-vision/tree/e0c24ed0fa57fa3e4f97f30de74c51d944036ace?recursive=true" \
  | jq -r '.[] | select(.lfs) | "\(.path) \(.lfs.oid)"'
```

## Updating a pin

Changing a model is a release, not a patch to a table. The commit and every
digest move together, `docs/models.md` and the constants are updated in the same
change, and `CHANGELOG.md` says which model moved and why — a store's vectors
are only comparable with vectors from the model that built it, so a model change
that nobody announced is a corpus that quietly stops agreeing with itself.
