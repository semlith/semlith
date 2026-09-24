# The portal drive

A scripted browser drive over the Chrome DevTools Protocol that replays the
reproduction for every finding of the 2026-09-17 full manual regression drive
and asserts the corrected behaviour.

The manual drive found 47 defects across 62 numbered findings and took most of
a day. This is the standing gate that would have caught all of them, and it
takes minutes.

The findings document it encodes lives outside this repository, at
`live-project-files/semlith/repos/semlith/full-regression-drive/17-09-2026/findings.md`. Read
it before touching `findings.py`: it describes the *buggy* behaviour, and every
check here asserts the *fixed* behaviour instead.

## Running it

```sh
semlith start                                 # in another terminal
python3 -m pip install websocket-client       # the one dependency
python3 tests/drive/drive.py
```

Output lands in `drive-out/`: a `transcript.txt` and one screenshot per check,
named `<id>-<slug-of-title>.png`. The exit status is the number of unexpected
failures plus the number of XPASSes, so CI can use it directly.

```sh
python3 tests/drive/drive.py --list             # every registered id
python3 tests/drive/drive.py --only 2.9,2.10    # a subset
python3 tests/drive/drive.py --out /tmp/drive   # somewhere else
python3 tests/drive/drive.py --keep-fixtures    # leave the corpora on disk
```

### What it needs

| | |
|---|---|
| Python 3.8 or newer | standard library, plus `websocket-client` |
| Chrome or Chromium | found per platform, or pointed at with `CHROME_PATH` |
| A running `semlith` daemon | the drive never starts one; it drives the one you are testing |
| `git` on PATH | the projects picker fixture builds two real repositories |
| Network access | only for the two checks that fetch a URL; they skip without it |

### Environment

| Variable | Default | What it does |
|---|---|---|
| `SEMLITH_PORTAL_URL` | `http://127.0.0.1:7365` | the daemon to drive |
| `SEMLITH_TOKEN` | discovered | the per-run session token |
| `SEMLITH_HOME` | `~/.semlith` | where the token is discovered from |
| `SEMLITH_BIN` | `semlith` | the binary the fixtures build a store with |
| `KNOWN_FAILURES` | `.github/known-failures.txt` | which failures are expected |
| `CHROME_PATH` | discovered | the browser binary |

When `SEMLITH_TOKEN` is unset the drive reads it the way `semlith mcp` does:
every store under `$SEMLITH_HOME/stores/*/daemon.json` carries the running
daemon's pid, port and current token (`Discovery` in `src/daemon.rs`). The
first file naming the port being driven wins.

## What it asserts

Every check is registered by its finding id, so `--only 3.14` runs exactly the
check for finding 3.14 and `transcript.txt` can be read next to the findings
document line by line.

Assertions go over whichever surface is honest for the finding:

* **HTTP**, with the `Semlith-Token` header, where the finding is about a
  status code, a JSON shape or a refusal — `2.10`, `3.19`, `4.13`, most of
  `1.1` and `1.2`.
* **The DOM**, through `eval`, where the finding is about layout, copy or what
  a button does — everything on the P3 and P4 lists.
* **Both**, where a fix has to land in two places. Finding `1.4` asserts that
  a finished run card offers Remove *and* that `POST /api/index/control` with
  `stop` refuses a run that is already over, because the invisible half of
  that bug is what latched the store and killed its next run in `1.3`.

Two product decisions are encoded here rather than inferred, and a check will
correctly fail if the implementation takes a different reading:

* A cross-store query writes one ledger row per contributing store, all
  sharing one query id. "Queries recorded" counts distinct query ids, so one
  search across N stores raises it by exactly one (`1.2`).
* Out-of-root rows are reconciled away when the daemon opens a store and again
  on `semlith index`, with a `--reconcile=report` dry run that counts without
  dropping. So by the time the portal can be asked there are none, which is
  what `1.1` asserts — not a badge saying there are some.

### Fixtures

`fixtures.py` builds every corpus from scratch under a temp directory, so the
drive does not depend on one developer's machine having an `ultraship`
checkout next door. Each is built on first use and removed at the end:

| | |
|---|---|
| `small()` | three files, the `proj-one` shape from findings 1.3 and 1.6 |
| `bulk()` | six hundred files, so the live chunk counter in 1.5 reaches a flush boundary |
| `adoptme()` | a folder holding a real `.semlith`, built by `SEMLITH_BIN`, for 2.1 and 4.18 |
| `monorepo()` | two git repositories and one plain folder, for the projects picker in 2.5 |
| `doomed()` | a corpus deleted out from under its own store, for the dead-entry badge in 3.1 |

`/api/dirs` confines the portal's pickers to the user's home directory, on
purpose (issue #71). The picker checks therefore need the fixtures inside it:
set `TMPDIR` (or `TEMP` on Windows) somewhere under `$HOME` before running the
drive if your system temp directory is elsewhere.

## Known failures

The runner reads `.github/known-failures.txt`, the same file `.github/smoke.sh`
and `.github/portal-check.ps1` read, in the same four-column format — id,
platform, issue, note — with the same platform tokens: `all`, `unix`,
`windows`, `linux`, `macos`.

```
3.6   all      104   graph label collision avoidance is not in 0.20.2
4.9   windows  118   the phone summary bar overlaps under DirectWrite metrics
```

A listed check that fails is reported `xfail` and does not fail the job. A
listed check that **passes** is reported `XPASS` and **does** fail the job, so
a fix cannot land without this file being updated in the same breath. That is
the whole point of it: without the XPASS rule, either the run is permanently
red and a new regression hides in the noise, or the known bugs get quietly
dropped from the harness and stop being tracked.

A check whose precondition did not hold — no browser, no network, a corpus
with no PDF in it — is reported `skip` and fails nothing. A harness that blames
the product for its own breakage is worse than no harness.

## Adding a check when the next drive finds finding 48

1. Add the finding to the drive's findings document first, with its
   reproduction. The document is the specification; this file is its
   executable form.
2. Add one function to `findings.py`, in finding order, decorated with its id
   and a one-line title written as a statement of the corrected behaviour —
   "a store holds no files outside its registered roots", not "out-of-root
   files". The title is what the CI log prints, and a title phrased as the bug
   reads as a failure even when it passes.
3. Assert the corrected behaviour, never the bug. Raise through `fail()` with
   a message naming what was expected and what was seen, and enough of the
   finding's reasoning that someone reading the CI log in a year knows why the
   assertion is there. Raise `skip()` when a precondition is not met.
4. If the corrected behaviour is ambiguous, take the reading the finding's own
   "should" sentence gives and write a comment naming the ambiguity, so the
   next person can see a choice was made.
5. Nothing uses a test framework and nothing raises a bare `assert`. Python is
   run with `-O` often enough, and an `AssertionError` with no message is a
   failure nobody can act on.

## When a check breaks because the markup moved

The DOM checks address the portal by role, by text and by shape rather than by
generated class names, but `src/portal/app.js` builds every control by hand and
a redesign will still move things. A check that can no longer find its element
fails with the selector it was looking for and a note saying so. Update the
check. Do not delete it, and do not weaken it into passing: the finding it
encodes is a bug that was really in a release, and the only thing standing
between it and the next one is this file.
