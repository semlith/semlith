"""ColGrep (LightOn next-plaid): ColBERT late-interaction (LateOn-Code-edge) fused
with FTS5 keyword search. The issue text is passed as-is: its docs take natural
language queries. Index lives in COLGREP_DATA_DIR=work, outside the repo."""

import json
import os

from . import _common as c

NAME = "ColGrep"
BIN = f"{c.TOOLS}/colgrep/colgrep-aarch64-apple-darwin/colgrep"
VERSION = "1.7.0"


def _env(work):
    return c.env(COLGREP_DATA_DIR=work)


def available():
    return None if os.access(BIN, os.X_OK) else f"colgrep not installed at {BIN}"


def version():
    return c.first_line([BIN, "--version"]).split()[-1]


def index(root, work):
    # Incremental: init re-encodes only added/changed files and drops deleted ones.
    os.makedirs(work, exist_ok=True)
    c.run([BIN, "init", "-y", root], cwd=root, env_=_env(work), timeout=c.INDEX_TIMEOUT)


def search(query, root, work, k=50):
    out = c.run([BIN, "--json", "-k", str(k), query, root], cwd=root, env_=_env(work))
    return [(c.rel(r["unit"]["file"], root), r["unit"]["line"], r["unit"]["end_line"], r["unit"].get("code", ""))
            for r in json.loads(out or "[]")]
