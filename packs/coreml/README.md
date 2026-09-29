# Core ML pack

The fp16 Core ML models behind semlith's Neural Engine and Core ML GPU lanes on
Apple silicon. CI builds and publishes the pack (`.github/workflows/packs.yml`,
`pack: coreml`) to the release `pack-coreml-v<PACK_VERSION>`; the binary pins it
by URL and SHA-256.

## Contents of `semlith-coreml-<PACK_VERSION>.zip`

One top-level directory, `semlith-coreml-<PACK_VERSION>/`, holding:

| Path | Layout | Batch | Buckets (tokens) | Compute units |
|---|---|---|---|---|
| `ane/ane_b4.mlmodelc`, function `s<S>` per bucket | ANE (B,C,1,S), 1x1 conv | 4 | 128, 192, 256, 320, 384, 512 | CPU_AND_NE |
| `gpu/std_b8.mlmodelc`, function `s<S>` per bucket | standard (B,S,C) | 8 | 128, 256, 512 | CPU_AND_GPU |
| `manifest.json` | `pack_version`, `minimum_macos`, `ane`/`gpu` {`batch`, `buckets`, `file`, `function` with a literal `{S}`} (what `src/coreml.rs` reads), plus HF repo + revision, `max_length`, input/output names and dtypes, pad token, `neg` | | | |

Every model: `ids` int32 (B,S), `kmask` fp16 (B,S) additive (0 keep, -1e4 pad)
→ `cls` fp32 (B,384), the raw CLS vector (L2-normalise it). Pad a batch to the
smallest bucket that fits; fill a short batch with rows whose `kmask` keeps
position 0 only, and discard them. The zip is deterministic (sorted entries,
1980-01-01 timestamps, mode 0644, deflate level 9, no directory entries or extra
fields), so two builds from the same pins are byte-identical.

## Build and check locally (Apple silicon, Python 3.11)

```sh
uv venv --python 3.11 /tmp/coreml-venv
uv pip install --python /tmp/coreml-venv/bin/python -r requirements.txt
/tmp/coreml-venv/bin/python convert.py --out /tmp/coreml-pack          # fetches the HF model at the pinned revision
/tmp/coreml-venv/bin/python placement.py /tmp/coreml-pack/semlith-coreml-1        # exits 1 if an ANE model puts < 95 % of ops on the Neural Engine
/tmp/coreml-venv/bin/python verify.py /tmp/coreml-pack/semlith-coreml-1           # cosine vs fp32 on tests/fixtures/coreml, chunks/s
```

`verify.py` reads the tokenizer from semlith's model cache (`--tokenizer` to
override). The first load of each ANE model compiles it for the Neural Engine
and takes minutes; macOS caches the result.

Change anything that alters the pack's bytes (models, `requirements.txt`) → bump
`PACK_VERSION` in `convert.py`, dispatch the workflow with that version, and
update the pinned URL and SHA-256 in the binary.
