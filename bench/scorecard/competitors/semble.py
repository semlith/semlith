"""Semble (MinishLab): Model2Vec potion-code-16M static embeddings + BM25, RRF-fused.
The issue text is passed as-is (its docs take natural language). Default content
type (code) as documented. Cache in SEMBLE_CACHE_LOCATION=work."""

import json
import os

from . import _common as c

NAME = "Semble"
BIN = f"{c.TOOLS}/venv-semble/bin/semble"
VERSION = "0.6.1"


def _env(work):
    return c.env(SEMBLE_CACHE_LOCATION=work)


def available():
    return None if os.access(BIN, os.X_OK) else f"semble not installed at {BIN}"


def version():
    return c.first_line([BIN, "-V"]).split()[-1]


def index(root, work):
    # No index command: the first search builds and caches the index, later
    # searches invalidate it per changed file. A throwaway query warms it.
    os.makedirs(work, exist_ok=True)
    c.run([BIN, "search", "-k", "1", "--max-snippet-lines", "0", "index", root],
          cwd=root, env_=_env(work), timeout=c.INDEX_TIMEOUT)


def search(query, root, work, k=50):
    out = c.run([BIN, "search", "-k", str(k), "--format", "json", query, root], cwd=root, env_=_env(work))
    return [(c.rel(r["file_path"], root), r["start_line"], r["end_line"], r.get("content", ""))
            for r in json.loads(out)["results"]]
