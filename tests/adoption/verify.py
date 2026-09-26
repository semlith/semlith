#!/usr/bin/env python3
"""Check that every fact refs.yaml rests on still holds on the tree it runs on.

A reference key written against one commit goes stale as the code moves, and
a stale key grades a correct answer down. Run this on the release tree before
judge.py:

    python3 tests/adoption/verify.py

For a fact at path:line with text, the line must contain the text. If it does
not, the nearest line that does is reported as MOVED, and a fact whose text is
nowhere in the file as GONE. A count fact re-counts matching lines under a
directory. Exit 0 only when every fact holds where it was cited; MOVED means
the key text needs its line numbers updated, GONE means it needs rewriting.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

import yaml

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path.insert(0, str(HERE))
from bench import PROMPTS  # noqa: E402


def count(root: Path, fact: dict) -> int:
    pattern = re.compile(fact["count"])
    exclude = re.compile(fact["not"]) if fact.get("not") else None
    n = 0
    for f in sorted((root / fact["under"]).rglob("*")):
        if not f.is_file() or (fact.get("ext") and f.suffix != fact["ext"]):
            continue
        if fact.get("skip") and fact["skip"] in f.relative_to(root).parts:
            continue
        for line in f.read_text(errors="replace").splitlines():
            if pattern.search(line) and not (exclude and exclude.search(line)):
                n += 1
    return n


def check(root: Path, fact: dict) -> tuple[str, str]:
    """(status, detail) for one fact."""
    if "count" in fact:
        n = count(root, fact)
        where = f"/{fact['count']}/ under {fact['under']}/"
        return ("OK", where) if n == fact["is"] else ("COUNT", f"{where}: {n} lines, key says {fact['is']}")
    path, _, line = fact["at"].partition(":")
    file = root / path
    if not file.is_file():
        return "GONE", f"{fact['at']}: no such file"
    if not line:
        return "OK", fact["at"]
    text = file.read_text(errors="replace").splitlines()
    want, cited = fact["has"], int(line)
    if 0 < cited <= len(text) and want in text[cited - 1]:
        return "OK", fact["at"]
    found = [i + 1 for i, t in enumerate(text) if want in t]
    if not found:
        return "GONE", f"{fact['at']}: {want!r} is nowhere in {path}"
    # A line that is exactly the text beats one that merely contains it: a
    # struct literal `Hit {` over the signature `-> Hit {` above it.
    whole = [i for i in found if text[i - 1].strip() == want.strip()]
    near = min(whole or found, key=lambda i: abs(i - cited))
    also = f" ({len(found)} lines have it)" if len(found) > 1 else ""
    return "MOVED", f"{fact['at']} -> {path}:{near}: {want!r}{also}"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--refs", default=str(HERE / "refs.yaml"))
    parser.add_argument("--all", action="store_true", help="print facts that hold too")
    args = parser.parse_args()

    doc = yaml.safe_load(Path(args.refs).read_text())
    repos = {name: (REPO / rel).resolve() for name, rel in doc["repos"].items()}
    refs = doc["refs"]
    tally = {"OK": 0, "MOVED": 0, "GONE": 0, "COUNT": 0, "SKIP": 0}

    for pid in sorted(set(PROMPTS) ^ set(refs)):
        print(f"KEY    {pid}: {'no reference key' if pid in PROMPTS else 'key for no prompt in bench.py'}")
        tally["GONE"] += 1

    for pid, ref in refs.items():
        lines = []
        for fact in ref.get("facts") or []:
            repo = fact.get("repo", "core")
            root = repos[repo]
            if not root.is_dir():
                status, detail = "SKIP", f"{repo} is not checked out at {root}"
            else:
                status, detail = check(root, fact)
            tally[status] += 1
            if status != "OK" or args.all:
                lines.append(f"  {status:6} {'' if repo == 'core' else repo + ': '}{detail}")
        print(f"{pid}: {len(ref.get('facts') or [])} facts" + ("" if lines else ", all hold"))
        if lines:
            print("\n".join(lines))

    print("\n" + " · ".join(f"{k} {v}" for k, v in tally.items()))
    return 0 if tally["OK"] and not (tally["MOVED"] or tally["GONE"] or tally["COUNT"]) else 1


if __name__ == "__main__":
    raise SystemExit(main())
