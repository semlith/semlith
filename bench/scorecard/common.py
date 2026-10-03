"""Shared pieces of the scorecard harness: paths, the excerpt type, budgets, the run manifest.

Everything the harness downloads, clones, indexes or writes lives under
SCORECARD_HOME (default ~/semlith-bench/scorecard), never in this repository.
"""
import hashlib, shutil, json, os, platform, subprocess, sys, time
from collections import namedtuple

HOME = os.environ.get("SCORECARD_HOME", os.path.expanduser("~/semlith-bench/scorecard"))
DATA = os.path.join(HOME, "data")
REPOS = os.path.join(HOME, "repos")
STORES = os.path.join(HOME, "stores")
RESULTS = os.path.join(HOME, "results")
SEMLITH = os.environ.get("SEMLITH_BIN", "semlith")

BUDGETS = (2000, 4000, 8000)
BYTES_PER_TOKEN = 4
RUNS = 3

# `path` is relative to the root the arm searched; `offset` is the UTF-8 byte
# offset of the excerpt's header in the arm's raw output, which is what a token
# budget is compared against (BYTES_PER_TOKEN bytes a token).
Excerpt = namedtuple("Excerpt", "path start end text offset")


def tokens_at(offset):
    return (offset + BYTES_PER_TOKEN - 1) // BYTES_PER_TOKEN


def within(excerpts, budget_tokens):
    limit = budget_tokens * BYTES_PER_TOKEN
    return [e for e in excerpts if e.offset < limit]


def ranked_files(excerpts):
    """Distinct files in first-appearance order."""
    seen, out = set(), []
    for e in excerpts:
        if e.path not in seen:
            seen.add(e.path)
            out.append(e.path)
    return out


def parquet_rows(path, columns=None):
    import pyarrow.parquet as pq  # the one dependency: `uv run --with pyarrow`
    return pq.read_table(path, columns=columns).to_pylist()


def sh(args, cwd=None, check=True, env=None, timeout=None):
    r = subprocess.run(args, cwd=cwd, capture_output=True, text=True, errors="replace", env=env, timeout=timeout)
    if check and r.returncode != 0:
        raise RuntimeError(f"{' '.join(args)} exited {r.returncode}: {r.stderr.strip()[-2000:]}")
    return r


def semlith_version():
    return sh([SEMLITH, "--version"]).stdout.strip()


def semlith_binary():
    """Which binary measured: its path and SHA-256, and the commit it was built from when SEMLITH_BIN_COMMIT says
    so. A branch build reports the last released version until its release commit, so the version string alone
    cannot tell two of them apart, and the repository's HEAD at the end of a run is not the commit a binary built
    at its start came from."""
    path = shutil.which(SEMLITH) or SEMLITH
    with open(path, "rb") as f:
        out = {"path": path, "sha256": hashlib.sha256(f.read()).hexdigest()}
    if os.environ.get("SEMLITH_BIN_COMMIT"):
        out["commit"] = os.environ["SEMLITH_BIN_COMMIT"]
    return out


def command_line():
    return "uv run --with pyarrow python " + " ".join([os.path.relpath(sys.argv[0])] + sys.argv[1:])


def run_dir(bench):
    d = os.path.join(RESULTS, bench, time.strftime("%Y%m%dT%H%M%S"))
    os.makedirs(d, exist_ok=True)
    return d


def write_manifest(directory, extra):
    files = {}
    for name in sorted(os.listdir(directory)):
        p = os.path.join(directory, name)
        if os.path.isfile(p) and name != "MANIFEST.json":
            with open(p, "rb") as f:
                files[name] = hashlib.sha256(f.read()).hexdigest()
    doc = {"command": command_line(), "semlith": semlith_version(), "semlith_binary": semlith_binary(), "machine": platform.platform(),
           "python": platform.python_version(), "written": time.strftime("%Y-%m-%dT%H:%M:%S%z"), "files": files}
    doc.update(extra)
    with open(os.path.join(directory, "MANIFEST.json"), "w") as f:
        json.dump(doc, f, indent=1, sort_keys=True)


def median_spread(values):
    v = sorted(values)
    return v[len(v) // 2], (v[-1] - v[0]) if v else 0
