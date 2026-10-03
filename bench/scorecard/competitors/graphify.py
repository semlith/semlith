"""Graphify (safishamsi/graphify, PyPI graphifyy): tree-sitter code graph built by
`graphify update` (code-only, no LLM; --no-cluster since query needs no
communities). Query = `graphify query "<issue text>"`, a BFS from the nodes the
question names; NODE lines are returned in the order printed. It writes
graphify-out/ in the project, so it indexes a mirror under work/.
GRAPHIFY_NO_AUTO_REFRESH=1 (set in _common.env) stops it rewriting
~/.claude/skills/graphify on every run."""

import os
import re

from . import _common as c

NAME = "Graphify"
BIN = f"{c.TOOLS}/venv-graphify/bin/graphify"
VERSION = "0.9.74"
KEEP = ("graphify-out",)
NODE = re.compile(r"^NODE (.*) \[src=(.*?) loc=L?(\d*)")


def _env():
    # GRAPHIFY_FORCE: after checking out an older commit the graph can shrink,
    # and update refuses to overwrite a graph with fewer nodes without it.
    return c.env(GRAPHIFY_FORCE="1")


def available():
    return None if os.access(BIN, os.X_OK) else f"graphify not installed at {BIN}"


def version():
    out = c.run([f"{c.TOOLS}/venv-graphify/bin/python", "-c",
                 "import importlib.metadata as m; print(m.version('graphifyy'))"])
    return out.strip()


def index(root, work):
    m = c.mirror(root, work, KEEP)
    c.run([BIN, "update", ".", "--no-cluster"], cwd=m, env_=_env(), timeout=c.INDEX_TIMEOUT)


def search(query, root, work, k=50):
    m = os.path.join(work, "mirror")
    budget = max(2000, 40 * k)  # 2000 is its default; ~25 tokens per NODE line
    out = c.run([BIN, "query", query, "--budget", str(budget), "--graph", "graphify-out/graph.json"],
                cwd=m, env_=_env())
    res = []
    for line in out.splitlines():
        g = NODE.match(line)
        if g and g.group(2):
            n = int(g.group(3) or 1)
            res.append((g.group(2), n, n, g.group(1)))
    return res[:k]
