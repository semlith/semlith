"""Download every dataset and repository the scorecard reads, into SCORECARD_HOME.

    uv run --with pyarrow python bench/scorecard/fetch.py [swe|coderag|repobench|all]

Sizes as of 2026-10-03: SWE-bench Lite 1.2 MB and Verified 2.1 MB (parquet);
the 12 SWE-bench repositories about 3 GB of full clones (history is needed to
check out each instance's base commit); CodeRAG-Bench's five task sets and two
corpora about 16 MB, RepoEval's tasks and repositories about 0.2 GB; RepoBench-R's test splits about 0.5 GB. Nothing is
downloaded twice.
"""
import os, shutil, sys, urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import DATA, REPOS, parquet_rows, sh

HF = "https://huggingface.co/api/datasets"
SWE = {"lite": "princeton-nlp/SWE-bench_Lite", "verified": "princeton-nlp/SWE-bench_Verified"}
CODERAG = ["humaneval", "mbpp", "ds1000", "odex", "library-documentation", "programming-solutions"]
REPOBENCH = [f"{lang}_{kind}/test_{level}" for lang in ("python", "java") for kind in ("cff", "cfr")
             for level in ("easy", "hard")]


def get(url, dest):
    if os.path.exists(dest):
        return dest
    os.makedirs(os.path.dirname(dest), exist_ok=True)
    tmp = dest + ".part"
    with urllib.request.urlopen(url) as r, open(tmp, "wb") as f:
        while chunk := r.read(1 << 20):
            f.write(chunk)
    os.replace(tmp, dest)
    print(f"fetched {dest} ({os.path.getsize(dest)} bytes)", flush=True)
    return dest


def parquet_list(dataset):
    import json
    with urllib.request.urlopen(f"{HF}/{dataset}/parquet") as r:
        return json.load(r)


def swe():
    repos = set()
    for name, ds in SWE.items():
        p = get(f"{HF}/{ds}/parquet/default/test/0.parquet", os.path.join(DATA, "swe", f"{name}.parquet"))
        repos |= {row["repo"] for row in parquet_rows(p, ["repo"])}
    for repo in sorted(repos):
        dest = os.path.join(REPOS, repo.replace("/", "__"))
        if not os.path.exists(dest):
            # Cloned beside the destination and renamed, so an interrupted clone
            # is never mistaken for a finished one.
            print(f"cloning {repo}", flush=True)
            part = dest + ".part"
            if os.path.exists(part):
                shutil.rmtree(part)
            sh(["git", "clone", "--quiet", f"https://github.com/{repo}.git", part])
            os.replace(part, dest)


# RepoEval's function-level tasks and repositories, from the same places CodeRAG-Bench's own
# retrieval/create/repoeval.py takes them (RepoCoder's datasets, and its corrected repositories).
REPOEVAL = {
    "datasets": "https://github.com/microsoft/CodeT/raw/main/RepoCoder/datasets/datasets.zip",
    "function_level": "https://github.com/Veronicium/repoeval_debug/raw/main/function_level.zip",
}


def coderag():
    for name in CODERAG:
        get(f"{HF}/code-rag-bench/{name}/parquet/default/train/0.parquet", os.path.join(DATA, "coderag", f"{name}.parquet"))
    import zipfile
    for name, url in REPOEVAL.items():
        dest = os.path.join(DATA, "repoeval", name)
        if not os.path.exists(dest):
            z = get(url, dest + ".zip")
            with zipfile.ZipFile(z) as f:
                f.extractall(dest + ".part")
            os.replace(dest + ".part", dest)


def repobench():
    listing = parquet_list("tianyang/repobench-r")
    for split in REPOBENCH:
        config, part = split.split("/")
        for i, url in enumerate(listing[config][part]):
            get(url, os.path.join(DATA, "repobench", config, f"{part}-{i}.parquet"))


if __name__ == "__main__":
    which = sys.argv[1] if len(sys.argv) > 1 else "all"
    for name, fn in (("swe", swe), ("coderag", coderag), ("repobench", repobench)):
        if which in (name, "all"):
            fn()
