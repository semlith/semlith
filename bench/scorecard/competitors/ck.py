"""ck (BeaconBay): semantic search (--sem, BGE-Small default model). Its docs
recommend `ck --jsonl --sem "<natural language>"` for NL queries; --hybrid adds
a regex pass, which for a whole issue as a regex matches nothing. --threshold 0
so the top-k is not cut by ck's 0.6 default score floor.
ck writes .ck/ next to the code, so it indexes a mirror under work/."""

import json
import os

from . import _common as c

NAME = "ck"
BIN = f"{c.TOOLS}/ck/ck"
VERSION = "0.7.11"
KEEP = (".ck",)


def _env():
    return c.env(FASTEMBED_CACHE_DIR=f"{c.TOOLS}/cache/fastembed")


def available():
    return None if os.access(BIN, os.X_OK) else f"ck not installed at {BIN}"


def version():
    return c.first_line([BIN, "--version"]).split()[-1]


def index(root, work):
    # Incremental: chunk-level embedding cache, only changed files re-embedded.
    m = c.mirror(root, work, KEEP)
    c.run([BIN, "--index", "."], cwd=m, env_=_env(), timeout=c.INDEX_TIMEOUT)


def search(query, root, work, k=50):
    m = os.path.join(work, "mirror")
    out = c.run([BIN, "--sem", "--jsonl", "--topk", str(k), "--threshold", "0", query, "."],
                cwd=m, env_=_env(), ok=(0, 1))
    res = []
    for line in out.splitlines():
        if line.startswith("{"):
            r = json.loads(line)
            res.append((c.rel(r["path"], m), r["span"]["line_start"], r["span"]["line_end"], r.get("snippet", "")))
    return res
