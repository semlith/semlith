#!/bin/sh
# Build granite as GGUF f16 for semlith's llama.cpp lane, from the pinned model
# revision with llama.cpp's own converter at the pinned build.
#
#   sh convert.sh OUT_DIR
#
# Writes OUT_DIR/granite-embedding-small-english-r2-f16.gguf and SHA256SUMS.
# Needs git and uv. Change anything here that alters the file's bytes → bump
# PACK_VERSION, dispatch the packs workflow with it, and update the pin in
# src/llama.rs.
set -eu

PACK_VERSION=1
LLAMA_TAG=b11146
HF_REPO=ibm-granite/granite-embedding-small-english-r2
HF_REVISION=2ab6fa8ea2d674564defd37171ae19079b864b33
NAME=granite-embedding-small-english-r2-f16.gguf

mkdir -p "$1"
out=$(cd "$1" && pwd)
rm -rf "$out/llama.cpp" "$out/venv" "$out/hf"
git -c advice.detachedHead=false clone -q --depth 1 --branch "$LLAMA_TAG" \
    https://github.com/ggml-org/llama.cpp "$out/llama.cpp"
uv venv -q --python 3.11 "$out/venv"
uv pip install -q --python "$out/venv/bin/python" \
    -r "$out/llama.cpp/requirements/requirements-convert_hf_to_gguf.txt"
"$out/venv/bin/python" - "$HF_REPO" "$HF_REVISION" "$out/hf" <<'PY'
import sys
from huggingface_hub import snapshot_download
snapshot_download(sys.argv[1], revision=sys.argv[2], local_dir=sys.argv[3])
PY
"$out/venv/bin/python" "$out/llama.cpp/convert_hf_to_gguf.py" "$out/hf" \
    --outtype f16 --outfile "$out/$NAME"
cd "$out"
if command -v sha256sum > /dev/null; then sha256sum "$NAME"; else shasum -a 256 "$NAME"; fi > SHA256SUMS
cat SHA256SUMS
wc -c < "$NAME"
