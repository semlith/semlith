"""ugrep: same query shape as rg (OR of identifier-like terms and 4+-letter
non-stopwords, one -e per term), files ranked by match count, 3 lines context.
--bool is not used: in it a space means AND, which a long issue never satisfies;
OR-ed -e patterns are what `--bool 'a|b|c'` would mean."""

import os

from . import _common as c

NAME = "ugrep"
BIN = f"{c.TOOLS}/ugrep/bin/ugrep"
VERSION = "7.8.5"
# -r recursive, -I skip binaries, --ignore-files honours .gitignore like rg does.
BASE = ["-r", "-I", "--ignore-files", "--exclude-dir=.git", "-i", "-F"]


def available():
    return None if os.access(BIN, os.X_OK) else f"ugrep not built at {BIN}"


def version():
    return c.first_line([BIN, "--version"]).split()[1]


def index(root, work):
    pass  # index-free


def search(query, root, work, k=50):
    terms = c.grep_terms(query)
    if not terms:
        return []
    pats = BASE + [a for t in terms for a in ("-e", t)]
    counts = []
    # -c -u counts every match (not just matching lines); -m1, drops zero-count files.
    for line in c.run([BIN, "-c", "-u", "-m1,", *pats, "."], cwd=root, ok=(0, 1)).splitlines():
        path, n = line.rsplit(":", 1)
        counts.append((-int(n), c.rel(path, root)))
    files = [p for _, p in sorted(counts)[:k]]
    if not files:
        return []
    hits = {p: [] for p in files}
    out = c.run([BIN, "-n", *pats, "--format=%f\t%n%~", "--", *files], cwd=root, ok=(0, 1))
    for line in out.splitlines():
        path, n = line.rsplit("\t", 1)
        hits[c.rel(path, root)].append(int(n))
    return [c.blocks(root, p, hits[p]) for p in files if hits[p]]
