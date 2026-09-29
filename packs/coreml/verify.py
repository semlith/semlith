"""Check a built pack against the fp32 reference on the pinned 512-chunk fixture.

    python verify.py PACK_DIR [--tokenizer tokenizer.json] [--fixture DIR]
    python verify.py --write-ref --hf SNAPSHOT_DIR   # regenerate ref_fp32.f32

For each layout in the manifest: load the compiled models with their compute
units (timing the first load: the ANE compiles on first use and caches it),
embed the fixture the way semlith will (sorted by length, batched, padded up to
the smallest bucket that fits), and report min/mean cosine vs the fp32
reference, bad rows (non-finite or all-zero) and chunks/s. Exits 1 if any
layout has a bad row or a min cosine below 0.999.
"""
import argparse
import json
import os
import statistics
import sys
import time

import numpy as np
from tokenizers import Tokenizer

HERE = os.path.dirname(os.path.abspath(__file__))
FIXTURE = os.path.join(HERE, '..', '..', 'tests', 'fixtures', 'coreml')
# The tokenizer semlith itself uses (the ONNX export's), from semlith's model cache.
TOKENIZER = os.path.expanduser(
    '~/.cache/semlith/models/models--onnx-community--granite-embedding-small-english-r2-ONNX/'
    'snapshots/1dc7835ba0cb9c76a3618d0bf0c427c97671b3c8/tokenizer.json')
MIN_COS = 0.999


def tokenize(path, texts, max_tokens):
    t = Tokenizer.from_file(path)
    t.enable_truncation(max_tokens)
    t.no_padding()
    return [e.ids for e in t.encode_batch(texts)]


def normalize(v):
    return v / np.linalg.norm(v, axis=1, keepdims=True)


def write_ref(hf, ids, dim, out):
    """CLS of the HF fp32 model, L2-normalised, 512 x dim little-endian f32."""
    import torch
    from transformers import AutoModel
    m = AutoModel.from_pretrained(hf, dtype=torch.float32).eval()
    ref = np.zeros((len(ids), dim), np.float32)
    order = np.argsort([len(i) for i in ids], kind='stable')
    with torch.no_grad():
        for s in range(0, len(order), 8):
            idx = order[s:s + 8]
            L = max(len(ids[i]) for i in idx)
            x = np.zeros((len(idx), L), np.int64)
            mask = np.zeros((len(idx), L), np.int64)
            for r, i in enumerate(idx):
                x[r, :len(ids[i])] = ids[i]
                mask[r, :len(ids[i])] = 1
            h = m(input_ids=torch.tensor(x), attention_mask=torch.tensor(mask)).last_hidden_state[:, 0]
            ref[idx] = h.numpy()
    normalize(ref).astype('<f4').tofile(out)


def jobs(ids, B, buckets, pad_id, neg):
    """(indices, feed) per batch: sorted by length, padded to the smallest bucket."""
    lens = np.array([len(i) for i in ids])
    order = np.argsort(lens, kind='stable')
    out = []
    for s in range(0, len(order), B):
        idx = order[s:s + B]
        S = next(b for b in buckets if b >= lens[idx].max())
        x = np.full((B, S), pad_id, np.int32)
        km = np.full((B, S), neg, np.float16)
        for r, i in enumerate(idx):
            x[r, :lens[i]] = ids[i]
            km[r, :lens[i]] = 0
        km[len(idx):, 0] = 0  # filler rows: never an all-pad row (NaN softmax)
        out.append((idx, S, {'ids': x, 'kmask': km}))
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('pack', nargs='?')
    ap.add_argument('--tokenizer', default=TOKENIZER)
    ap.add_argument('--fixture', default=FIXTURE)
    ap.add_argument('--reps', type=int, default=3)
    ap.add_argument('--write-ref', action='store_true')
    ap.add_argument('--hf', help='HF snapshot, for --write-ref')
    a = ap.parse_args()

    texts = json.load(open(os.path.join(a.fixture, 'chunks.json')))
    ref_path = os.path.join(a.fixture, 'ref_fp32.f32')
    if a.write_ref:
        ids = tokenize(a.tokenizer, texts, 400)
        write_ref(a.hf, ids, 384, ref_path)
        print('wrote', ref_path)
        return

    import coremltools as ct
    manifest = json.load(open(os.path.join(a.pack, 'manifest.json')))
    dim = manifest['dim']
    ids = tokenize(a.tokenizer, texts, manifest['max_length'])
    ref = np.fromfile(ref_path, '<f4').reshape(-1, dim)
    assert len(ref) == len(texts), 'reference and fixture disagree on chunk count'
    neg = manifest['neg']

    results, ok = [], True
    for sub in ('ane', 'gpu'):
        lay = manifest[sub]
        units = getattr(ct.ComputeUnit, lay['compute_units'])
        t = time.perf_counter()
        models = {S: ct.models.CompiledMLModel(os.path.join(a.pack, lay['file'].replace('{S}', str(S))),
                                               compute_units=units,
                                               function_name=lay.get('function', 'main').replace('{S}', str(S)))
                  for S in lay['buckets']}
        load_s = time.perf_counter() - t
        work = jobs(ids, lay['batch'], lay['buckets'], manifest['pad_token_id'], neg)

        def run():
            out = np.zeros((len(ids), dim), np.float32)
            for idx, S, feed in work:
                out[idx] = models[S].predict(feed)['cls'][:len(idx)]
            return out

        t = time.perf_counter()
        out = run()  # first predict per model also pays device warm-up
        first_s = time.perf_counter() - t
        rates = []
        for _ in range(a.reps):
            t = time.perf_counter()
            out = run()
            rates.append(len(ids) / (time.perf_counter() - t))
        good = np.isfinite(out).all(1) & (np.abs(out).sum(1) > 0)
        cos = (normalize(out[good]) * ref[good]).sum(1)
        r = dict(layout=sub, compute_units=lay['compute_units'], batch=lay['batch'],
                 load_s=round(load_s, 2), first_pass_s=round(first_s, 2),
                 chunks_s=round(statistics.median(rates), 1), runs=[round(x, 1) for x in rates],
                 cos_min=round(float(cos.min()), 6), cos_mean=round(float(cos.mean()), 6),
                 bad_rows=int((~good).sum()))
        r['passed'] = r['bad_rows'] == 0 and r['cos_min'] >= MIN_COS
        ok &= r['passed']
        results.append(r)
        print(json.dumps(r), flush=True)
    sys.exit(0 if ok else 1)


if __name__ == '__main__':
    main()
