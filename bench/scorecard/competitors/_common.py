"""Shared plumbing for the competitor adapters: tool paths, a sandboxed env,
subprocess calls with timeouts, the repo mirror, and issue -> term extraction."""

import os
import re
import subprocess

TOOLS = os.environ.get("SCORECARD_TOOLS", "/Users/aakashpawar/semlith-bench/scorecard/tools")
INDEX_TIMEOUT = 2 * 3600
SEARCH_TIMEOUT = 120


def env(**extra):
    """Every tool runs with HOME and caches redirected under tools/, so nothing
    lands in the real home directory (or the repo under test)."""
    e = dict(os.environ)
    e.update(
        HOME=f"{TOOLS}/home",
        HF_HOME=f"{TOOLS}/cache/hf",
        XDG_CACHE_HOME=f"{TOOLS}/cache/xdg",
        XDG_CONFIG_HOME=f"{TOOLS}/home/.config",
        XDG_DATA_HOME=f"{TOOLS}/home/.local/share",
        GRAPHIFY_NO_AUTO_REFRESH="1",
    )
    e.update(extra)
    return e


def run(cmd, timeout=SEARCH_TIMEOUT, cwd=None, env_=None, ok=(0,)):
    p = subprocess.run(cmd, cwd=cwd, env=env_ or env(), stdin=subprocess.DEVNULL,
                       capture_output=True, text=True,
                       errors="replace", timeout=timeout)
    if p.returncode not in ok:
        raise RuntimeError(f"{os.path.basename(cmd[0])} exit {p.returncode}: {(p.stderr or p.stdout)[-2000:]}")
    return p.stdout


def first_line(cmd):
    try:
        return run(cmd, timeout=30).strip().splitlines()[0]
    except Exception as e:  # version is informational only
        return f"unknown ({e})"


def mirror(root, work, keep=()):
    """rsync the working tree (no .git) into work/mirror for tools that write
    their index inside the project. Mtimes are preserved, so after a checkout
    only the changed files look changed. `keep` names the tool's own index
    dirs, which rsync must neither copy nor delete."""
    dst = os.path.join(work, "mirror")
    os.makedirs(dst, exist_ok=True)
    cmd = ["rsync", "-a", "--delete", "--exclude=.git"] + [f"--exclude={k}" for k in keep]
    run(cmd + [root.rstrip("/") + "/", dst + "/"], timeout=INDEX_TIMEOUT)
    return dst


def rel(path, base):
    return os.path.relpath(path, base) if os.path.isabs(path) else path.removeprefix("./")


# 4+-letter English and issue-boilerplate words that carry no retrieval signal.
STOP = set("""
about above after again against also although always among another anything around because been before
being below between both cannot could currently does doing done down during each either else even every
example expected following from further getting going have having hello here however instead into issue
itself just know like look looks maybe might more most much must need needs only other otherwise ought over
please probably quite rather really same seem seems should since some something still such sure than thank
thanks that their them then there these they thing think this those though through trying under until upon
used uses using very want wants well were what when where whether which while will with within without work
works would your yours actually already problem happens happen behavior behaviour simply
""".split())

_IDENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*")


def identifiers(text):
    """Identifier-like tokens: snake_case, dotted.names, camelCase/CapsWords,
    backticked code and anything called like f(...)."""
    ticked = set(_IDENT.findall(" ".join(re.findall(r"`([^`]+)`", text))))
    out, seen = [], set()
    for m in _IDENT.finditer(text):
        t = m.group(0)
        parts = t.split(".")
        if len(t) < 3 or all(len(p) < 2 for p in parts):
            continue
        called = text[m.end():m.end() + 1] == "("
        if ("_" in t.strip("_") or len(parts) > 1 or re.search(r"[a-z][A-Z]", t)
                or t in ticked or called):
            if t not in seen:
                seen.add(t)
                out.append(t)
    return out


def words(text, exclude=()):
    ex = {e.lower() for e in exclude}
    out, seen = [], set()
    for w in re.findall(r"[A-Za-z]{4,}", text):
        w = w.lower()
        if w not in STOP and w not in ex and w not in seen:
            seen.add(w)
            out.append(w)
    return out


def grep_terms(text):
    """rg/ugrep query: identifiers first, then 4+-letter non-stopwords (OR-ed)."""
    ids = identifiers(text)
    return ids + words(text, exclude=ids)


def blocks(root, path, lines, ctx=3, max_chars=4000):
    """One result per file: matched lines with `ctx` lines of context, merged
    into contiguous blocks (as grep -C prints them)."""
    try:
        with open(os.path.join(root, path), errors="replace") as f:
            src = f.read().splitlines()
    except OSError:
        return (path, lines[0], lines[-1], "")
    spans = []
    for n in sorted(set(lines)):
        a, b = max(1, n - ctx), min(len(src), n + ctx)
        if spans and a <= spans[-1][1] + 1:
            spans[-1][1] = max(spans[-1][1], b)
        else:
            spans.append([a, b])
    text = "\n--\n".join("\n".join(src[a - 1:b]) for a, b in spans)[:max_chars]
    return (path, spans[0][0], spans[-1][1], text)


if __name__ == "__main__":
    q = "Session.request ignores timeout when proxies set; see `merge_environment_settings` and getProxy()"
    assert identifiers(q) == ["Session.request", "merge_environment_settings", "getProxy"], identifiers(q)
    assert words(q, identifiers(q))[:4] == ["session", "request", "ignores", "timeout"], words(q)
    assert "when" not in grep_terms(q) and "timeout" in grep_terms(q)
    print("ok")
