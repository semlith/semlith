"""Build the Core ML pack: convert every model, compile each to .mlmodelc, write
manifest.json, then a deterministic semlith-coreml-<PACK_VERSION>.zip and SHA256SUMS.

    python convert.py --out DIR [--hf SNAPSHOT_DIR]

DIR/semlith-coreml-<PACK_VERSION>/ is the unpacked pack (and the zip's single
top-level directory); DIR/work/ holds the intermediate .mlpackage files.
Without --hf the model is fetched from the Hub at the pinned revision.
"""
import argparse
import hashlib
import json
import os
import shutil
import sys
import zipfile
import time

import coremltools as ct
import numpy as np
import torch
import transformers
from huggingface_hub import snapshot_download

from granite_ane import NEG, Granite

# Bump when the pack's bytes change for any reason (models, pins, layout): the
# binary pins the archive by name and SHA-256, so a changed pack is a new pack.
PACK_VERSION = 1
HF_REPO = 'ibm-granite/granite-embedding-small-english-r2'
HF_REVISION = '2ab6fa8ea2d674564defd37171ae19079b864b33'
MAX_TOKENS = 400  # semlith truncates chunks at 800 chars / 2 tokens per char
MIN_MACOS = '15.0'  # ct.target.macOS15, the target the bench converted and measured at

# (pack dir, layout, batch, buckets, compute units). The ANE runs batch 4 fastest
# and wants fine buckets because it pays for every padded position; the GPU
# prefers bigger batches and cares less about padding.
GROUPS = [
    ('ane', 'ane', 4, [128, 192, 256, 320, 384, 512], 'CPU_AND_NE'),
    ('gpu', 'std', 8, [128, 256, 512], 'CPU_AND_GPU'),
]


def model_name(layout, B, S):
    return f'{layout}_b{B}_s{S}'


def strip_metadata(ml):
    """coremltools stamps a conversion date and writes its metadata as a protobuf
    map in an order that changes run to run: both make the compiled model's
    coremldata.bin differ between identical builds. One fixed key instead."""
    meta = ml.user_defined_metadata
    for k in list(meta.keys()):
        del meta[k]
    meta['semlith.built_with'] = f'torch=={torch.__version__} coremltools=={ct.__version__}'


def convert_one(hf, layout, B, S, units, pkg):
    g = Granite(hf, S, layout).eval()
    ids = torch.zeros((B, S), dtype=torch.int32)
    km = torch.zeros((B, S))
    with torch.no_grad():
        traced = torch.jit.trace(g, (ids, km))
    ml = ct.convert(
        traced,
        inputs=[ct.TensorType('ids', (B, S), np.int32), ct.TensorType('kmask', (B, S), np.float16)],
        outputs=[ct.TensorType('cls', dtype=np.float32)],
        compute_precision=ct.precision.FLOAT16,
        minimum_deployment_target=ct.target.macOS15,
        convert_to='mlprogram',
        compute_units=getattr(ct.ComputeUnit, units),
        # Loading here would compile for the ANE (minutes) and fails on runners
        # without one; the compiled pack is loaded by placement.py / verify.py.
        skip_model_load=True,
    )
    strip_metadata(ml)
    ml.save(pkg)


def build_zip(src, dest):
    """src itself is the zip's single top-level directory. Deterministic: sorted
    entries, fixed timestamp, mode and compression level, no directory entries
    (extractors create parents) and no extra fields."""
    base = os.path.dirname(src)
    paths = []
    for root, dirs, files in os.walk(src):
        for name in dirs + files:
            path = os.path.join(root, name)
            if os.path.islink(path) or not (os.path.isdir(path) or os.path.isfile(path)):
                sys.exit(f'refusing to pack non-regular file {path}')
            if os.path.isfile(path):
                paths.append(path)
    with zipfile.ZipFile(dest, 'w') as z:
        for path in sorted(paths, key=lambda p: os.path.relpath(p, base)):
            zi = zipfile.ZipInfo(os.path.relpath(path, base), date_time=(1980, 1, 1, 0, 0, 0))
            zi.create_system = 3  # unix, so external_attr carries the mode on every host
            zi.external_attr = 0o100644 << 16
            with open(path, 'rb') as f:
                z.writestr(zi, f.read(), compress_type=zipfile.ZIP_DEFLATED, compresslevel=9)


def sha256(path):
    h = hashlib.sha256()
    with open(path, 'rb') as f:
        for block in iter(lambda: f.read(1 << 20), b''):
            h.update(block)
    return h.hexdigest()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--out', required=True)
    ap.add_argument('--hf', help='local snapshot of the HF model at the pinned revision')
    a = ap.parse_args()

    hf = a.hf or snapshot_download(HF_REPO, revision=HF_REVISION,
                                   allow_patterns=['config.json', 'model.safetensors'])
    config = json.load(open(os.path.join(hf, 'config.json')))
    name = f'semlith-coreml-{PACK_VERSION}'
    work, pack = os.path.join(a.out, 'work'), os.path.join(a.out, name)
    for d in (work, pack):
        shutil.rmtree(d, ignore_errors=True)
        os.makedirs(d)

    # One model per layout with a function per bucket: the buckets share every
    # weight, and a multifunction model stores them once. Nine single-function
    # models made a 662 MB pack.
    for sub, layout, B, buckets, units in GROUPS:
        os.makedirs(os.path.join(pack, sub))
        desc = ct.utils.MultiFunctionDescriptor()
        for S in buckets:
            pkg = os.path.join(work, model_name(layout, B, S) + '.mlpackage')
            t = time.perf_counter()
            convert_one(hf, layout, B, S, units, pkg)
            print(f'{sub} s{S}: convert {time.perf_counter() - t:.1f}s', flush=True)
            desc.add_function(pkg, src_function_name='main', target_function_name=f's{S}')
        desc.default_function_name = f's{buckets[0]}'
        raw = os.path.join(work, f'{layout}_b{B}-raw.mlpackage')
        merged = os.path.join(work, f'{layout}_b{B}.mlpackage')
        ct.utils.save_multifunction(desc, raw)
        # Saved elsewhere: a model saved over its own package deletes it first.
        ml = ct.models.MLModel(raw, skip_model_load=True)
        strip_metadata(ml)
        ml.save(merged)
        rel = f'{sub}/{layout}_b{B}.mlmodelc'
        t = time.perf_counter()
        ct.utils.compile_model(merged, os.path.join(pack, rel))
        print(f'{rel}: compile {time.perf_counter() - t:.1f}s', flush=True)

    # src/coreml.rs deserialises pack_version, minimum_macos and ane/gpu.{batch,
    # buckets,file,function} (`{S}` is a literal placeholder); the rest is
    # description.
    manifest = dict(
        pack='coreml',
        pack_version=str(PACK_VERSION),
        minimum_macos=MIN_MACOS,
        hf_repo=HF_REPO,
        hf_revision=HF_REVISION,
        max_length=MAX_TOKENS,
        dim=config['hidden_size'],
        pad_token_id=config['pad_token_id'],
        neg=NEG,
        pooling='cls',  # output is the raw CLS vector; L2-normalise it
        inputs=dict(ids=dict(dtype='int32', shape=['batch', 'seq']),
                    kmask=dict(dtype='float16', shape=['batch', 'seq'], keep=0.0, pad=NEG)),
        outputs=dict(cls=dict(dtype='float32', shape=['batch', 'dim'])),
        # A batch with fewer chunks than `batch` is filled with rows whose kmask is
        # keep at position 0 and pad elsewhere: an all-pad row would softmax to NaN.
        filler_row='kmask keep at position 0, pad elsewhere; discard its output',
        built_with={'torch': torch.__version__, 'transformers': transformers.__version__,
                    'coremltools': ct.__version__, 'numpy': np.__version__},
    )
    for sub, layout, B, buckets, units in GROUPS:
        manifest[sub] = dict(batch=B, buckets=buckets, file=f'{sub}/{layout}_b{B}.mlmodelc',
                             function='s{S}', layout=layout, compute_units=units)
    with open(os.path.join(pack, 'manifest.json'), 'w') as f:
        json.dump(manifest, f, indent=2, sort_keys=True)
        f.write('\n')

    archive = name + '.zip'
    build_zip(pack, os.path.join(a.out, archive))
    digest = sha256(os.path.join(a.out, archive))
    with open(os.path.join(a.out, 'SHA256SUMS'), 'w') as f:
        f.write(f'{digest}  {archive}\n')
    print(json.dumps(dict(archive=os.path.join(a.out, archive), sha256=digest,
                          bytes=os.path.getsize(os.path.join(a.out, archive)))))


if __name__ == '__main__':
    main()
