"""Run every available adapter on tools/testrepo with one query and print key,
version, index seconds, search seconds and the top 5 files.
Usage (from bench/scorecard): python3.11 -m competitors.smoke [key ...]"""

import os
import sys
import time

from . import ALL
from ._common import TOOLS

ROOT = f"{TOOLS}/testrepo"
QUERY = "Session.request ignores timeout when proxies set"


def main(keys):
    for key in keys or ALL:
        m = ALL[key]
        why = m.available()
        if why:
            print(f"{key:10} not run: {why}", flush=True)
            continue
        work = f"{TOOLS}/work/{key}/testrepo"
        os.makedirs(work, exist_ok=True)
        try:
            t = time.time()
            m.index(ROOT, work)
            ti = time.time() - t
            t = time.time()
            res = m.search(QUERY, ROOT, work, k=50)
            ts = time.time() - t
        except Exception as e:
            print(f"{key:10} FAILED: {type(e).__name__}: {str(e)[:300]}", flush=True)
            continue
        top = list(dict.fromkeys(p for p, *_ in res))[:5]
        print(f"{key:10} {m.version():10} index {ti:7.1f}s  search {ts:5.2f}s  {len(res):3} results  top5: {top}",
              flush=True)


if __name__ == "__main__":
    main(sys.argv[1:])
