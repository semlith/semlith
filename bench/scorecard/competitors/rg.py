"""ripgrep: OR of the issue's identifier-like terms and 4+-letter non-stopwords,
files ranked by match count, matched lines with 3 lines of context."""

import json
import shutil

from . import _common as c

NAME = "ripgrep"
BIN = shutil.which("rg") or "/opt/homebrew/bin/rg"


def available():
    return None if shutil.which(BIN) else f"rg not found at {BIN}"


def version():
    return c.first_line([BIN, "--version"]).split()[1]


VERSION = "15.2.0"


def index(root, work):
    pass  # index-free


def search(query, root, work, k=50):
    terms = c.grep_terms(query)
    if not terms:
        return []
    pats = ["-i", "-F"] + [a for t in terms for a in ("-e", t)]
    counts = []
    for line in c.run([BIN, "--count-matches", *pats, "."], cwd=root, ok=(0, 1)).splitlines():
        path, n = line.rsplit(":", 1)
        counts.append((-int(n), c.rel(path, root)))
    files = [p for _, p in sorted(counts)[:k]]
    if not files:
        return []
    hits = {p: [] for p in files}
    for line in c.run([BIN, "--json", *pats, "--", *files], cwd=root, ok=(0, 1)).splitlines():
        m = json.loads(line)
        if m["type"] == "match":
            hits[m["data"]["path"]["text"]].append(m["data"]["line_number"])
    return [c.blocks(root, p, hits[p]) for p in files if hits[p]]
