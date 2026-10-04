"""Sourcebot: self-hosted Zoekt + universal-ctags (Docker Compose stack in
tools/sourcebot, official compose file adapted; anonymous access forced on so
the search API needs no account). Query = the issue's identifier-like terms and
4+-letter non-stopwords, each quoted as a literal keyword, OR-ed and scoped
with repo:; files in the order Zoekt ranks them.

Sourcebot indexes a local repo's default branch, never a detached HEAD, and
treats the repo as read-only. So index() keeps a `git clone --no-checkout
--no-hardlinks` of the root under work/git (a full copy of .git; --shared
alternates are invisible to zoekt's go-git, which then indexes nothing),
fetches the root's HEAD into it (reading the root only), points a branch there
at that commit, and waits for Sourcebot to re-index it (reindexIntervalMs is
15 s in config.json)."""

import json
import os
import re
import shutil
import subprocess
import time
import urllib.request

from . import _common as c

NAME = "Sourcebot"
VERSION = "5.1.15"
DOCKER = shutil.which("docker") or "/usr/local/bin/docker"
DIR = f"{c.TOOLS}/sourcebot"
CONFIG = f"{DIR}/data/config.json"
MOUNT = "/Users/aakashpawar/semlith-bench/scorecard"  # mounted read-only at the same path
URL = "http://127.0.0.1:3070"
BRANCH = "scorecard"


def _host_env():
    return dict(os.environ)  # docker needs the real ~/.docker context


def _api(path, body=None, timeout=c.SEARCH_TIMEOUT):
    req = urllib.request.Request(URL + path, data=json.dumps(body).encode() if body is not None else None,
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.load(r)


def _up():
    try:
        _api("/api/repos", timeout=10)
        return True
    except OSError:
        return False


def available():
    if not os.access(DOCKER, os.X_OK):
        return f"docker not found at {DOCKER}"
    try:
        c.run([DOCKER, "version", "--format", "{{.Server.Version}}"], timeout=30, env_=_host_env())
    except Exception as e:
        return f"docker daemon not reachable: {e}"
    return None


def version():
    try:
        _ensure_up()
        return str(_api("/api/version").get("version", VERSION)).lstrip("v")
    except Exception:
        return VERSION


def _ensure_up():
    if _up():
        return
    c.run([DOCKER, "compose", "-p", "scorecard-sourcebot", "up", "-d"], cwd=DIR, env_=_host_env(),
          timeout=c.INDEX_TIMEOUT)
    for _ in range(300):
        if _up():
            return
        time.sleep(2)
    raise RuntimeError("sourcebot did not come up on " + URL)


def _git(*args, cwd):
    return c.run(["git", *args], cwd=cwd, env_=_host_env(), timeout=c.INDEX_TIMEOUT).strip()


def _repo_name(origin):
    # Sourcebot names a generic git repo host/path from remote.origin.url.
    s = re.sub(r"^[a-z+]+://", "", origin).removesuffix(".git")
    s = re.sub(r"^[^@/]+@", "", s).replace(":", "/", 1) if "://" not in origin else re.sub(r"^[^@/]+@", "", s)
    return s


def _indexed_at(name):
    for r in _api("/api/repos?perPage=100"):
        if r["repoName"] == name and r.get("indexedAt"):
            return r["indexedAt"]
    return None


def index(root, work):
    root = os.path.realpath(root)
    if not (root + "/").startswith(MOUNT + "/"):
        raise RuntimeError(f"{root} is outside the read-only mount {MOUNT}")
    _ensure_up()
    sha = _git("rev-parse", "HEAD", cwd=root)
    origin = _git("config", "remote.origin.url", cwd=root)
    # A clone of a local clone names a filesystem path as its origin, which Sourcebot's URL parser rejects
    # ("URL parsing failed") so the repository never indexes. Follow local origins up to the real remote.
    while os.path.isdir(origin):
        origin = _git("config", "remote.origin.url", cwd=origin)
    clone = os.path.join(work, "git")
    if not os.path.isdir(clone):
        os.makedirs(work, exist_ok=True)
        subprocess.run(["git", "clone", "-q", "--no-checkout", "--no-hardlinks", root, clone], check=True,
                       env=_host_env(), timeout=c.INDEX_TIMEOUT)
    _git("config", "remote.origin.url", origin, cwd=clone)
    _git("fetch", "-q", "--no-tags", root, "HEAD", cwd=clone)
    _git("update-ref", f"refs/heads/{BRANCH}", sha, cwd=clone)
    _git("symbolic-ref", "HEAD", f"refs/heads/{BRANCH}", cwd=clone)
    name = _repo_name(origin)
    with open(work + "/sourcebot.json", "w") as f:
        json.dump({"repoName": name, "sha": sha}, f)

    with open(CONFIG) as f:
        cfg = json.load(f)
    conn = os.path.basename(work.rstrip("/"))
    want = {"type": "git", "url": "file://" + clone}
    t0 = time.strftime("%Y-%m-%dT%H:%M:%S", time.gmtime())
    if cfg["connections"].get(conn) != want:
        cfg["connections"][conn] = want
        with open(CONFIG + ".tmp", "w") as f:
            json.dump(cfg, f, indent=2)
        os.replace(CONFIG + ".tmp", CONFIG)
    # Wait for two completed index runs after the ref moved: the first may have
    # started before it did.
    deadline, seen = time.time() + c.INDEX_TIMEOUT, []
    while time.time() < deadline:
        at = _indexed_at(name)
        if at and at > t0 and at not in seen:
            seen.append(at)
            if len(seen) >= 2:
                return
        time.sleep(2)
    raise TimeoutError(f"sourcebot did not re-index {name}")


def _quote(t):
    return '"' + t.replace("\\", "\\\\").replace('"', '\\"') + '"'


def search(query, root, work, k=50):
    with open(work + "/sourcebot.json") as f:
        name = json.load(f)["repoName"]
    terms = c.grep_terms(query)
    if not terms:
        return []
    q = f"repo:^{re.escape(name)}$ (" + " or ".join(_quote(t) for t in terms) + ")"
    res = _api("/api/search", {"query": q, "matches": 5000, "contextLines": 3})
    out = []
    for f in res.get("files", [])[:k]:
        chunks = f.get("chunks") or []
        if not chunks:
            continue
        # Chunks come in Zoekt's score order, not line order.
        start = min(ch["contentStart"]["lineNumber"] for ch in chunks)
        end = max(ch["contentStart"]["lineNumber"] + ch["content"].rstrip("\n").count("\n") for ch in chunks)
        text = "\n--\n".join(ch["content"] for ch in chunks)[:4000]
        out.append((f["fileName"]["text"], start, end, text))
    return out
