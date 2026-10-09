"""grepai (yoanbernabeu): embeddings via its default provider, Ollama with
nomic-embed-text (the only local choice besides the LM Studio GUI), GOB store.
The issue text is passed as-is (natural-language search). grepai has no one-shot
index: `grepai watch` does an initial scan, then watches; index() runs it until
it reports the scan done, then stops it. It keeps .grepai/ in the project, so it
indexes a mirror under work/. Its default test-path penalties are left on."""

import json
import os
import signal
import subprocess
import time
import urllib.request

from . import _common as c

NAME = "grepai"
BIN = f"{c.TOOLS}/grepai/grepai"
OLLAMA = f"{c.TOOLS}/ollama/ollama"
HOST = "127.0.0.1:11434"  # grepai's default endpoint
MODEL = "nomic-embed-text"
VERSION = "0.37.0"
KEEP = (".grepai",)


def _ollama_env():
    return c.env(OLLAMA_MODELS=f"{c.TOOLS}/cache/ollama-models", OLLAMA_HOST=HOST)


def _ollama_up():
    try:
        with urllib.request.urlopen(f"http://{HOST}/api/tags", timeout=3) as r:
            return json.load(r)
    except OSError:
        return None


def _ensure_ollama():
    if _ollama_up() is None:
        os.makedirs(f"{c.TOOLS}/logs", exist_ok=True)
        subprocess.Popen([OLLAMA, "serve"], env=_ollama_env(), stdin=subprocess.DEVNULL,
                         stdout=open(f"{c.TOOLS}/logs/ollama.log", "a"), stderr=subprocess.STDOUT,
                         start_new_session=True)
        for _ in range(60):
            if _ollama_up() is not None:
                break
            time.sleep(1)
    tags = _ollama_up() or {}
    if not any(m["name"].startswith(MODEL) for m in tags.get("models", [])):
        c.run([OLLAMA, "pull", MODEL], env_=_ollama_env(), timeout=c.INDEX_TIMEOUT)


def available():
    for b in (BIN, OLLAMA):
        if not os.access(b, os.X_OK):
            return f"not installed: {b}"
    return None


def version():
    return c.first_line([BIN, "version"]).split()[-1]


def index(root, work):
    _ensure_ollama()
    m = c.mirror(root, work, KEEP)
    if not os.path.exists(os.path.join(m, ".grepai", "config.yaml")):
        c.run([BIN, "init", "--provider", "ollama", "--backend", "gob", "--yes"], cwd=m)
    # Incremental: the scan skips files whose content is unchanged and removes deleted ones.
    p = subprocess.Popen([BIN, "watch", "--no-ui"], cwd=m, env=c.env(), stdin=subprocess.DEVNULL,
                         stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, errors="replace",
                         start_new_session=True)
    deadline, log = time.time() + c.INDEX_TIMEOUT, []
    try:
        for line in p.stdout:
            log.append(line[-300:])
            if "Watching for changes" in line:
                return
            if time.time() > deadline:
                raise TimeoutError("grepai watch: initial scan exceeded the index timeout")
        raise RuntimeError("grepai watch exited before the scan finished: " + "".join(log[-20:]))
    finally:
        if p.poll() is None:
            os.killpg(p.pid, signal.SIGINT)
            try:
                p.wait(30)
            except subprocess.TimeoutExpired:
                os.killpg(p.pid, signal.SIGKILL)


def search(query, root, work, k=50):
    _ensure_ollama()
    m = os.path.join(work, "mirror")
    out = c.run([BIN, "search", "--json", "-n", str(k), query], cwd=m)
    return [(r["file_path"], r["start_line"], r["end_line"], r.get("content", "")) for r in json.loads(out or "[]")]
