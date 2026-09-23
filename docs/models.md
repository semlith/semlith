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

### The fp16 and fp32 exports

From 0.28.0 two more exports of the same model are pinned, at the same commit
and with the same tokenizer files. They are recorded in `src/embed.rs` as
`Variant::files`, and the fp16 pair is also in `src/gpu.rs` as `FP16_FILES`,
with the sizes the download is checked against.

| Variant | File | SHA-256 | Bytes |
| --- | --- | --- | --- |
| fp16 | `onnx/model_fp16.onnx` | `ee200de55cb2f94e858aabca54be7697a9c0805a14c858ee26ad0922b05f57d7` | 200 792 |
| fp16 | `onnx/model_fp16.onnx_data` | `28d16e29cd623f25cc6fa0968700c5bc31036466091a5fa06d1353c1777f050e` | 97 402 880 |
| fp32 | `onnx/model.onnx` | `cddb145cd1147ec24a3908b2ca2602b98b20a3d198365cff270b7cb26c98179e` | |
| fp32 | `onnx/model.onnx_data` | `86a3a705d4598615894d89540ea71a3d9bbdb17a315e79edcd5dfc737222834b` | |

**fp16 is what the GPU lanes run.** The int8 graph does not load on WebGPU. The
fp16 pair is downloaded with the WebGPU plugin, the first time a run starts on a
machine that has a hardware GPU with the GPU lane on, or when CUDA is turned on.
Both files are stored in the plugin's own directory in the model cache, not in
the Hugging Face snapshot. Tested on the 2026-09-23 corpus, fp16 and int8
vectors of the same text agree at cosine 0.987. A store records how many chunks
each variant embedded, in its `variants` meta row.

**fp32 is what the known-answer fixture was made with.** `tests/fixtures/gpu/`
holds 32 chunks and their CPU fp32 vectors. `semlith doctor --gpu`, and every
GPU lane before its first real batch, compare against them. The product never
downloads fp32. It is fetched only when a harness asks for it with
`SEMLITH_EMBED_VARIANT=fp32`, which is not part of the documented environment.

## Accelerator components

These are not models, but they are pinned and verified the same way. Each
download is streamed through SHA-256 and deleted if its digest does not match.
Each set is kept under `accel/` in the model cache, in a directory named by its pinned
version. Turning a lane off leaves the directory in place, and `semlith accel
remove <gpu | cuda>` deletes it. Under `--airgap` each download is refused
unless its directory has been seeded beforehand.

### The WebGPU plugin

Microsoft's WebGPU plugin execution provider, `onnxruntime-ep-webgpu` 0.4.0,
taken from the wheel Microsoft publishes on PyPI. It is MIT-licensed. semlith
extracts the plugin library, plus the two shader compilers beside it on Windows,
into `accel/webgpu-0.4.0/` in the model cache. Recorded in `src/gpu.rs` as
`WEBGPU_VERSION` and `wheel()`.

| Platform | Wheel | SHA-256 | Bytes |
| --- | --- | --- | --- |
| macOS (universal2) | [`onnxruntime_ep_webgpu-0.4.0-py3-none-macosx_14_0_universal2.whl`](https://files.pythonhosted.org/packages/c3/9f/1e09865bbe4c9202ff1334545535be32dfdbeffc46e7d4c36c5de67ffafe/onnxruntime_ep_webgpu-0.4.0-py3-none-macosx_14_0_universal2.whl) | `8d0a91d44b43d931c3068b9134aed9770f27803e353c2b58c93e1700bb68bcca` | 5 200 679 |
| Linux x86_64 | [`onnxruntime_ep_webgpu-0.4.0-py3-none-manylinux_2_28_x86_64.whl`](https://files.pythonhosted.org/packages/c1/96/2a18a45079250afcd825687aef2895de266d5abc1cff9f52d2dede7598f2/onnxruntime_ep_webgpu-0.4.0-py3-none-manylinux_2_28_x86_64.whl) | `5d8c961eda91a88961cad1fb7621f389f006b18497d6ea9ecf96f729f066cbfb` | 6 892 120 |
| Linux aarch64 | [`onnxruntime_ep_webgpu-0.4.0-py3-none-manylinux_2_28_aarch64.whl`](https://files.pythonhosted.org/packages/4e/ca/00c70322c19913c81a6bb2239aca83781b97a818b55c2ec3d25cec72dc4c/onnxruntime_ep_webgpu-0.4.0-py3-none-manylinux_2_28_aarch64.whl) | `17f660db53b1a509c63e721ac6784a2daa1c4e91a968523ee9986505ee5b0a55` | 6 096 222 |
| Windows x86_64 | [`onnxruntime_ep_webgpu-0.4.0-py3-none-win_amd64.whl`](https://files.pythonhosted.org/packages/d7/a4/c98a9e9433b3eeb576977b26c5c1cd0364f15ba9d198bb16101e7563ab06/onnxruntime_ep_webgpu-0.4.0-py3-none-win_amd64.whl) | `7db646669d1a2390551da675115ea125e68cb3d4d2c3a2d6e463372bf8d6ea87` | 13 149 847 |

It is fetched only after a hardware adapter has been found. A machine whose
only adapter is a software renderer (Mesa lavapipe or llvmpipe, SwiftShader,
Microsoft WARP or the Basic Render Driver) counts as having no GPU, and nothing
is downloaded.

### The CUDA pack

x86_64 Linux only in this release. Pack `1.24.4-cu12.8`, recorded in
`src/cuda.rs` as `PACK_VERSION` and `PARTS`, with `PACK_BYTES` asserted against
the sum of the sizes in `tests/cuda.rs`. It is downloaded only after CUDA has
been turned on, with `semlith accel on cuda` or the switch on the Machine
limits card, and the size is stated before the download starts. When the
system supplies none of the libraries, the download is 1 891 522 804 bytes
(1.89 GB). A library the system already has, at the pinned version or a newer
one in the same major version, is used from the system, and its wheel is not
fetched. The parts are listed in load order.

| Part | Version | Source | SHA-256 | Bytes | Supplies |
| --- | --- | --- | --- | --- | --- |
| ONNX Runtime GPU build | 1.24.4 | [GitHub release](https://github.com/microsoft/onnxruntime/releases/download/v1.24.4/onnxruntime-linux-x64-gpu-1.24.4.tgz) | `c5f804ff5d239b436fa59e9f2fb288a39f7eb9552f6a636c8b71e792e91a8808` | 205 429 115 | `libonnxruntime.so.1.24.4`, `libonnxruntime_providers_shared.so`, `libonnxruntime_providers_cuda.so` |
| `nvidia-cuda-runtime-cu12` | 12.8.90 | [PyPI wheel](https://files.pythonhosted.org/packages/0d/9b/a997b638fcd068ad6e4d53b8551a7d30fe8b404d6f1804abf1df69838932/nvidia_cuda_runtime_cu12-12.8.90-py3-none-manylinux2014_x86_64.manylinux_2_17_x86_64.whl) | `adade8dcbd0edf427b7204d480d6066d33902cab2a4707dcfc48a2d0fd44ab90` | 954 765 | `libcudart.so.12` |
| `nvidia-nvjitlink-cu12` | 12.8.93 | [PyPI wheel](https://files.pythonhosted.org/packages/f6/74/86a07f1d0f42998ca31312f998bd3b9a7eff7f52378f4f270c8679c77fb9/nvidia_nvjitlink_cu12-12.8.93-py3-none-manylinux2010_x86_64.manylinux_2_12_x86_64.whl) | `81ff63371a7ebd6e6451970684f916be2eab07321b73c9d244dc2b4da7f73b88` | 39 254 836 | `libnvJitLink.so.12` |
| `nvidia-cublas-cu12` | 12.8.4.1 | [PyPI wheel](https://files.pythonhosted.org/packages/dc/61/e24b560ab2e2eaeb3c839129175fb330dfcfc29e5203196e5541a4c44682/nvidia_cublas_cu12-12.8.4.1-py3-none-manylinux_2_27_x86_64.whl) | `8ac4e771d5a348c551b2a426eda6193c19aa630236b418086020df5ba9667142` | 594 346 921 | `libcublasLt.so.12`, `libcublas.so.12` |
| `nvidia-cufft-cu12` | 11.3.3.83 | [PyPI wheel](https://files.pythonhosted.org/packages/1f/13/ee4e00f30e676b66ae65b4f08cb5bcbb8392c03f54f2d5413ea99a5d1c80/nvidia_cufft_cu12-11.3.3.83-py3-none-manylinux2014_x86_64.manylinux_2_17_x86_64.whl) | `4d2dd21ec0b88cf61b62e6b43564355e5222e4a3fb394cac0db101f2dd0d4f74` | 193 118 695 | `libcufft.so.11` |
| `nvidia-curand-cu12` | 10.3.9.90 | [PyPI wheel](https://files.pythonhosted.org/packages/fb/aa/6584b56dc84ebe9cf93226a5cde4d99080c8e90ab40f0c27bda7a0f29aa1/nvidia_curand_cu12-10.3.9.90-py3-none-manylinux_2_27_x86_64.whl) | `b32331d4f4df5d6eefa0554c565b626c7216f87a06a4f56fab27c3b68a830ec9` | 63 619 976 | `libcurand.so.10` |
| `nvidia-cuda-nvrtc-cu12` | 12.8.93 | [PyPI wheel](https://files.pythonhosted.org/packages/05/6b/32f747947df2da6994e999492ab306a903659555dddc0fbdeb9d71f75e52/nvidia_cuda_nvrtc_cu12-12.8.93-py3-none-manylinux2010_x86_64.manylinux_2_12_x86_64.whl) | `a7756528852ef889772a84c6cd89d41dfa74667e24cca16bb31f8f061e3e9994` | 88 040 029 | `libnvrtc-builtins.so.12.8`, `libnvrtc.so.12` |
| `nvidia-cudnn-cu12` | 9.10.2.21 | [PyPI wheel](https://files.pythonhosted.org/packages/ba/51/e123d997aa098c61d029f76663dedbfb9bc8dcf8c60cbd6adbe42f76d049/nvidia_cudnn_cu12-9.10.2.21-py3-none-manylinux_2_27_x86_64.whl) | `949452be657fa16687d0930933f032835951ef0892b37d2d53824d1a84dc97a8` | 706 758 467 | `libcudnn.so.9` and its seven sub-libraries |

The NVIDIA versions are the CUDA 12.8 set that PyTorch 2.8's cu128 wheels pin.
In practice, ONNX Runtime's `onnxruntime-gpu[cuda,cudnn]` extras resolve to the
same set. The card must run a driver of 525.60.13 or newer, the minimum for CUDA
12.x on Linux x86_64. A card on an older driver is reported with that minimum
and is not used. On Windows, an NVIDIA card is used through WebGPU on D3D12.

**Licences.** ONNX Runtime is Microsoft's MIT-licensed release, downloaded from
its GitHub release. None of NVIDIA's libraries pass through semlith. The user's
machine downloads each wheel from `files.pythonhosted.org`, where NVIDIA
publishes it. These are the same bytes from the same place that `pip install
nvidia-cudnn-cu12` would fetch, and semlith never hosts, mirrors or ships a
copy. The user's use of the libraries is governed by the licence each wheel
carries ("NVIDIA Proprietary Software"). These are the terms a pip install of
the same wheel accepts. When a wheel includes that licence text, semlith keeps
it beside the libraries in `licences/`. semlith does not redistribute the
libraries, so the redistribution terms in the CUDA Toolkit EULA and the cuDNN
Software License Agreement do not apply to it.

## Rescoring: jina-reranker-v1-turbo-en

A cross-encoder, 37 M parameters, int8, Apache-2.0. It reads the query and a
candidate together and reorders the head of the fused list; it embeds nothing
and adds no candidates, so a store's vectors do not depend on it.

**It is off unless you turn it on**, with `SEMLITH_RERANK=on`, and the reason
is measured: a search over one store takes 8.2 ms, and 132.2 ms with this stage
over twelve candidates. What that buys is two questions at k=1 and one at k=3
of seventy-seven. Worth it when an answer matters more than a tenth of a
second; not worth making every agent's every search sixteen times slower by
default.

`semlith setup` fetches it beside the embedding model so that turning it on
never pauses a query to download one, and `semlith stats` and `semlith doctor`
both say which ranking a search used.

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
