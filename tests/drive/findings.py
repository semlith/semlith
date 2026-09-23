"""One check per finding from the 2026-09-17 full regression drive.

The findings document describes what the 0.20.1 portal did wrong. Every check
here asserts what 0.20.2 should do instead, so a check that passes means the
bug is gone and a check that fails means it is back.

Each check is registered by its finding id:

    @finding("1.1", "a store holds no files outside its registered roots")
    def _(d): ...

`d` is a `cdp.Drive`: a headless browser plus an HTTP client for the portal,
both already holding the session token. Assertions go over whichever surface
is honest for the finding. A status code, a JSON shape or a `limit` refusal is
asserted over HTTP; a layout, a piece of copy or what a button does is
asserted through the DOM.

Two conventions worth knowing before you edit this file:

  * Nothing raises a bare `AssertionError`. Every failure names what was
    expected and what was seen, because the person reading it is looking at a
    CI log six months from now and has no other context.
  * A check whose precondition could not be met raises `Skipped`, not a
    failure. A harness that blames the product for a missing browser, a
    missing network or a missing git is worse than no harness. This is the
    same rule `.github/smoke.sh` states at the top of itself.

Where the corrected behaviour was genuinely ambiguous — the findings document
says what is wrong, not always what right looks like — the check takes the
reading the finding's own "should" sentence gives, and a comment names the
ambiguity so the next person can see the choice was made rather than assumed.
"""

import json
import os
import re
import time
import urllib.parse

import cdp
import fixtures

CHECKS = []


# The portal builds every control itself, so a selector here is only ever worth
# what `src/portal/app.js` actually writes. These four are named rather than
# repeated because several checks share them and a guessed selector that
# matches nothing turns a check into a check of nothing.

#: One run's card on the Index page — `el("div", {class: "card pad run-card"})`.
RUN_CARD = ".run-card"

#: The folder picker or the projects checklist, whichever is currently open.
#: Both are `el("div", {class: "card picker", hidden: true})`, and the Index
#: page's reveal closes one before opening the other, so at most one matches.
OPEN_PICKER = ".card.picker:not([hidden])"

#: The Search page's query box — `labelled("search-query", …)` sets the id.
SEARCH_BOX = "#search-query"

#: One file's block of hits on the Search page — `class: "locate-group"`.
RESULT_CARD = ".locate-group"


class CheckFailed(Exception):
    """The corrected behaviour is not there."""


class Skipped(Exception):
    """The check could not run, which is not the product's fault."""


def finding(finding_id, title):
    def register(function):
        CHECKS.append((finding_id, title, function))
        return function

    return register


def fail(message):
    raise CheckFailed(message)


def skip(reason):
    raise Skipped(reason)


def want(what, seen, expected):
    """The one assertion helper. Every failure reads the same way."""
    if seen != expected:
        fail("%s: expected %r, saw %r" % (what, expected, seen))


# ------------------------------------------------------------------ DOM sugar


def text_of(d, selector, what):
    """The visible text of one element, or a failure naming the selector."""
    value = d.eval(
        "(() => { const el = document.querySelector(%s);"
        " return el ? (el.innerText || '').trim() : null; })()" % json.dumps(selector)
    )
    if value is None:
        fail(
            "%s: nothing matched %r. If the portal's markup moved, update this "
            "check rather than deleting it." % (what, selector)
        )
    return value


def texts_of(d, selector):
    """The visible text of every matching element, in document order."""
    return d.eval(
        "[...document.querySelectorAll(%s)].map(el => (el.innerText || '').trim())"
        % json.dumps(selector)
    )


def exists(d, selector):
    return bool(d.eval("!!document.querySelector(%s)" % json.dumps(selector)))


def view_text(d):
    """Everything the current view says, for copy assertions."""
    return d.eval("(document.querySelector('#root') || document.body).innerText")


def picker_where(d):
    """The folder the open picker is currently showing."""
    return text_of(d, OPEN_PICKER + " .where", "the open picker's current folder")


def descend_picker(d, path):
    """Walk the open picker from wherever it is down to `path`.

    Clicking the rows it lists is the only way in. `/api/dirs` and
    `/api/projects` confine both pickers to the home directory, and the card's
    own `open` is a closure with no handle on it from outside the page, so a
    check that wants the picker pointed somewhere has to point it the way a
    person would.
    """
    here = picker_where(d)
    # Compared as text rather than with `os.path.relpath`, which refuses two
    # paths it reads as being on different mounts — and `\\?\C:\...` and
    # `C:\...` are exactly that to Python, though they are one drive.
    here_key, path_key = normalise(here), normalise(path)
    if path_key == here_key:
        return
    if not path_key.startswith(here_key.rstrip("/") + "/"):
        fail(
            "the picker is showing %s, which is not above %s, so there is no way "
            "to walk down to it" % (here, path)
        )
    # The segments come from the path as it is spelled, not from the key. The
    # key is lowercased so that `C:` and `c:` compare equal; clicking a row
    # called `AppData` with the text `appdata` matches nothing.
    depth = len([p for p in here_key.rstrip("/").split("/") if p])
    spelled = [p for p in path.replace("\\", "/").rstrip("/").split("/") if p]
    for part in spelled[depth:]:
        if part in ("", "."):
            continue
        clicked = d.eval(
            """
            (() => {
              const picker = document.querySelector(%s);
              if (!picker) return 'no picker is open';
              for (const entry of picker.querySelectorAll('.entry')) {
                const name = entry.querySelector('.name');
                // The name alone, not the row's text: a row can also carry an
                // "in <store>" pill or a store badge beside the folder's name.
                if (name && name.textContent === %s) {
                  entry.scrollIntoView({block: "center"});
                  entry.click();
                  return 'clicked';
                }
              }
              return 'nothing in the listing is named ' + %s;
            })()
            """
            % (json.dumps(OPEN_PICKER), json.dumps(part), json.dumps(part))
        )
        if clicked != "clicked":
            fail("walking the picker down to %s stopped at %s: %s" % (path, here, clicked))
        here = os.path.join(here, part)
        d.wait_for(
            "(document.querySelector(%s) || {}).innerText === %s"
            % (json.dumps(OPEN_PICKER + " .where"), json.dumps(here)),
            what="the picker to open %s" % here,
        )
    return here


#: The "runs at once" field. `settingField` gives each of the three an
#: `aria-label` and no name or id, so the label is the handle.
RUNS_AT_ONCE = 'input[aria-label="runs at once"]'


def limits_panel(d):
    """Everything the machine limits panel says, with its panel already open."""
    text = d.eval(
        "(() => { const field = document.querySelector(%s);"
        " return field ? field.closest('.card').innerText : null; })()"
        % json.dumps(RUNS_AT_ONCE)
    )
    if text is None:
        fail(
            "the machine limits panel is open and holds no %s field. The three "
            "numbers are 'runs at once', 'threads each' and 'MiB per store'."
            % RUNS_AT_ONCE
        )
    return text


def rects(d, selector):
    """The bounding boxes of matching elements, for layout assertions."""
    return d.eval(
        """
        [...document.querySelectorAll(%s)].map(el => {
          const r = el.getBoundingClientRect();
          return {top: r.top, left: r.left, bottom: r.bottom, right: r.right,
                  width: r.width, height: r.height,
                  text: (el.innerText || '').trim().slice(0, 40)};
        })
        """
        % json.dumps(selector)
    )


# ----------------------------------------------------------- portal sugar


def stores(d):
    return d.api("/api/stores")["stores"]


def store_named(d, name):
    for row in stores(d):
        if row["name"] == name:
            return row
    fail("no store named %r is open. Stores present: %s"
         % (name, ", ".join(sorted(r["name"] for r in stores(d)))))


def doctor_state(row):
    """The state the Doctor page's `semlith` column shows for one client row.

    `/api/doctor` returns the facts — `note`, `registered`, `scope`, `command`,
    `present` — and `doctorView()`'s own `state(c)` in `src/portal/app.js`
    derives the words from them. There is no `state` field on the row, so a
    check that read one grouped every client under `''` and then compared their
    `command` values, which are the client binaries (`claude`, `codex`, …) and
    not remedies at all. This is that function, in the same order.
    """
    if row.get("note"):
        return "cannot register"
    if row.get("registered"):
        return "registered (%s)" % (row.get("scope") or "user")
    if not row.get("command"):
        return "not registered"
    if not row.get("present"):
        return "not installed"
    if row.get("scope") == "project":
        return "one project only"
    return "installed, not registered"


def start_index(d, path):
    """Index a folder as its own store, and return (run id, store name).

    `store: "each"` is used rather than naming a store because it is the one
    submission shape that both creates the store and tells the caller what it
    was called, which makes every later assertion about that store exact.
    """
    answer = d.api("/api/index", method="POST", body={"path": [path], "store": "each"})
    runs = answer.get("runs") or []
    if not runs or "error" in runs[0]:
        fail("indexing %s was refused: %s" % (path, json.dumps(answer)[:300]))
    return runs[0]["run"], runs[0]["store"]


def run_for(d, store_name):
    """The newest run the daemon holds for a store, or None.

    Runs are keyed by id and a store can have several, so "the store's run" is
    the most recent of them rather than whichever the route listed first — the
    first is usually one that finished several checks ago.
    """
    mine = [run for run in d.api("/api/index/runs")["runs"] if run["store"] == store_name]
    if not mine:
        return None
    return max(mine, key=lambda run: run.get("id") or 0)


def run_by_id(d, run_id):
    for run in d.api("/api/index/runs")["runs"]:
        if run.get("id") == run_id:
            return run
    return None


TERMINAL = {"done", "stopped", "failed"}

#: How long a submitted run may take to appear on `/api/index/runs` at all.
#: A run that never appears was never admitted, and no amount of further
#: waiting will change that.
RUN_APPEARS = 30

#: How long a fixture run may take to finish. The corpora this drive builds are
#: three files and six hundred files; neither is minutes of work, and a wait
#: long enough to cover a pathological machine is long enough to stall every
#: check behind it. One check used to wait ten minutes for a run that never
#: started and took the whole drive down with it.
RUN_FINISHES = 240


def wait_for_run(d, store_name, timeout=RUN_FINISHES, run_id=None):
    """Block until a run is over, and return its final snapshot.

    Bounded twice, because the two ways a run fails to finish need different
    messages. A run that never appears at all was refused a slot rather than
    started, and saying so is most of the diagnosis.
    """
    look = (lambda: run_by_id(d, run_id)) if run_id is not None else (lambda: run_for(d, store_name))
    what = "run %s on %s" % (run_id, store_name) if run_id is not None else "the run on %s" % store_name

    appears = time.time() + RUN_APPEARS
    while time.time() < appears:
        if look() is not None:
            break
        time.sleep(0.5)
    else:
        fail(
            "%s never appeared on /api/index/runs within %ds. The submission was "
            "accepted, so the run exists somewhere and was never admitted — which "
            "is what an admission slot leaked by an earlier stop looks like."
            % (what, RUN_APPEARS)
        )

    deadline = time.time() + timeout
    last = None
    while time.time() < deadline:
        last = look()
        if last and str(last.get("status", "")).lower() in TERMINAL:
            return last
        time.sleep(0.5)
    fail(
        "%s never finished within %ds (last status: %s, %s of %s files). Bounded "
        "deliberately: a check that waits indefinitely stalls every check after it."
        % (
            what,
            timeout,
            last and last.get("status"),
            last and last.get("scanned"),
            last and last.get("total"),
        )
    )


def indexed_fixture(d, path):
    """Index a fixture folder and wait for it, returning the store name."""
    run_id, store_name = start_index(d, path)
    wait_for_run(d, store_name, run_id=run_id)
    return store_name


def all_files(d, params=""):
    """Every row of /api/files, paged, so a check can reason about the corpus."""
    rows = []
    offset = 0
    while True:
        page = d.api("/api/files?limit=500&offset=%d%s" % (offset, params))
        rows.extend(page["files"])
        if len(page["files"]) < 500 or offset + 500 >= page["total"]:
            return rows
        offset += 500


def normalise(path):
    """One spelling of a path, so a Windows comparison is about the path.

    The verbatim prefix comes off first. `registry.json` holds what
    `std::fs::canonicalize` produced, which on Windows is `\\?\C:\...`, and
    that is deliberate — it is what the long-path APIs need. A comparison
    against a path a person typed has to meet it in the middle.
    """
    text = path.replace("\\", "/").rstrip("/").lower()
    for prefix in ("//?/unc/", "//?/"):
        if text.startswith(prefix):
            text = text[len(prefix):]
            if prefix == "//?/unc/":
                text = "//" + text
            break
    return text


# ==========================================================================
# P1 — data or trust is wrong
# ==========================================================================


@finding("1.1", "a store holds no files outside its registered roots")
def _(d):
    # The decision this encodes: out-of-root rows are reconciled away when the
    # daemon opens a store and again on `semlith index`. So by the time the
    # portal can be asked, there are none — not a badge saying there are some.
    roots = {}
    for row in stores(d):
        roots[row["name"]] = [normalise(r["path"]) for r in row.get("roots", [])]

    strays = []
    duplicates = []
    seen = set()
    for row in all_files(d):
        store, path = row["store"], normalise(row["path"])
        registered = roots.get(store, [])
        if registered and not any(path.startswith(root + "/") or path == root for root in registered):
            strays.append("%s claims %s, whose roots are %s" % (store, row["path"], registered))
        if (store, path) in seen:
            duplicates.append("%s holds %s twice" % (store, row["path"]))
        seen.add((store, path))

    if strays:
        fail(
            "%d indexed files fall outside their store's registered roots; the "
            "daemon reconciles these away on open and on index, so there should "
            "be none. First few:\n  %s" % (len(strays), "\n  ".join(strays[:5]))
        )
    if duplicates:
        fail("a store lists the same path twice:\n  %s" % "\n  ".join(duplicates[:5]))


@finding("1.2", "one cross-store search raises the ledger's query count by exactly one")
def _(d):
    # The decision this encodes: a cross-store query writes one ledger row per
    # contributing store, all sharing one query id, and "queries recorded"
    # counts distinct query ids. So the count moves with what the agent did,
    # never with how many stores happen to be open.
    #
    # Both stores are built here rather than hoped for. This drive gets its own
    # empty store home, so when this check runs nothing is open at all and a
    # skip would quietly retire a P1 gate. Two corpora, because indexing one
    # directory twice makes one store and one store shows no multiplier.
    indexed_fixture(d, d.fixtures.small())
    indexed_fixture(d, d.fixtures.second())

    open_stores = [row for row in stores(d) if not row.get("unopened")]
    if len(open_stores) < 2:
        fail(
            "two fixture corpora were indexed and the daemon reports %d open "
            "store(s): %s. The cross-store ledger count cannot be asserted "
            "against one store."
            % (len(open_stores), ", ".join(sorted(r["name"] for r in open_stores)))
        )

    before = d.api("/api/ledger")["queries"]
    d.api("/api/search?query=release%20record%20sealed%20immutable&k=8")
    # The write is on the request's own path, but the ledger read below is a
    # separate connection; one short settle beats a retry loop nobody reads.
    time.sleep(1.0)
    after = d.api("/api/ledger")["queries"]

    if after != before + 1:
        fail(
            "one search across %d stores moved QUERIES RECORDED from %d to %d. "
            "A cross-store query is one query id, so the count should have "
            "risen by exactly 1, not by the number of open stores."
            % (len(open_stores), before, after)
        )


@finding("1.3", "a URL fetched into a store is indexed, not left orphaned")
def _(d):
    store_name = indexed_fixture(d, d.fixtures.small())
    before = store_named(d, store_name)["files"]

    status, answer = d.api_result(
        "/api/add", method="POST", body={"url": "https://example.com", "store": store_name}
    )
    if status != 200:
        message = json.dumps(answer)
        if any(word in message.lower() for word in ("dns", "resolve", "network", "timed out", "connect")):
            skip("this machine cannot reach the public internet: %s" % message[:160])
        fail("fetching a URL into %s was refused with %d: %s" % (store_name, status, message[:300]))

    final = wait_for_run(d, store_name)
    want("the run a URL fetch starts", str(final["status"]).lower(), "done")
    after = store_named(d, store_name)["files"]
    if after != before + 1:
        fail(
            "the page was fetched but not indexed: %s went from %d files to %d. "
            "The whole point of `add` is to make the URL searchable."
            % (store_name, before, after)
        )


@finding("1.4", "a finished run card offers Remove, never Pause or Stop")
def _(d):
    store_name = indexed_fixture(d, d.fixtures.small())
    d.open_view("index")
    d.wait_for(
        "[...document.querySelectorAll(%s)].some(c => c.innerText.includes(%s))"
        % (json.dumps(RUN_CARD), json.dumps(store_name)),
        what="the finished run card for %s" % store_name,
    )

    # Only the buttons the card is actually offering. A finished card keeps its
    # Pause and its Stop in the markup and sets `hidden` on them, so a reader
    # that took every `button` in the card would report controls nobody can see.
    buttons = d.eval(
        """
        (() => {
          const cards = [...document.querySelectorAll(%s)];
          const card = cards.find(c => /done|stopped|failed/i.test(c.innerText)
                                    && c.innerText.includes(%s));
          if (!card) return null;
          return [...card.querySelectorAll('button')]
            .filter(b => !b.hidden && b.offsetParent !== null)
            .map(b => (b.innerText || '').trim());
        })()
        """
        % (json.dumps(RUN_CARD), json.dumps(store_name))
    )
    if buttons is None:
        fail("no run card for %s in a terminal state was found on the Index page" % store_name)
    # The fold toggle a finished card starts with; it is not a run control.
    buttons = [b for b in buttons if b not in ("Show the log", "Hide the log")]

    live = [b for b in buttons if b.lower() in ("pause", "stop", "resume")]
    if live:
        fail(
            "a finished run card still offers %s. A run that is over has nothing "
            "to pause or stop; it should offer Remove." % ", ".join(live)
        )
    if not any(b.lower() == "remove" for b in buttons):
        fail("a finished run card offers %r and no Remove" % buttons)

    # And the route agrees, so a stale page cannot latch the store either.
    status, answer = d.api_result(
        "/api/index/control", method="POST", body={"store": store_name, "action": "stop"}
    )
    if status == 200:
        fail(
            "POST /api/index/control stop on a store whose run is already over "
            "answered 200. It should refuse, because the latch it leaves is what "
            "killed the store's next run in finding 1.3. Saw: %s" % json.dumps(answer)[:200]
        )


@finding("1.5", "the live chunk counter never goes backwards during a run")
def _(d):
    run_id, store_name = start_index(d, d.fixtures.bulk())
    high = 0
    samples = []
    deadline = time.time() + 900
    final = None
    while time.time() < deadline:
        run = run_for(d, store_name)
        if run is None:
            time.sleep(0.5)
            continue
        chunks = run.get("chunks") or 0
        samples.append((run.get("indexed"), chunks))
        if chunks < high:
            fail(
                "the live chunk counter fell from %d to %d part way through the "
                "run (at %s files). It reports the run total, so it can only "
                "climb; a per-flush batch window shown in its place makes a long "
                "run look like it is losing work. Samples: %s"
                % (high, chunks, run.get("indexed"), samples[-6:])
            )
        high = max(high, chunks)
        if str(run.get("status", "")).lower() in TERMINAL:
            final = run
            break
        time.sleep(1.0)

    if final is None:
        fail("the bulk run never finished; %d samples taken" % len(samples))
    held = store_named(d, store_name)["chunks"]
    if final.get("chunks") != held:
        fail(
            "the run finished reporting %s chunks and the store holds %s. The "
            "final number was right in 0.20.1 and is the one thing that must "
            "stay right." % (final.get("chunks"), held)
        )


@finding("1.6", "a run card's totals agree with the store it wrote")
def _(d):
    # The ambiguity: the findings explain the under-report by a race with the
    # watcher, which had already embedded two of three files before the run
    # acquired the writer. Both a fix that makes the run count that work and a
    # fix that keeps the watcher off a store with a queued run satisfy the
    # finding's complaint, which is that the card and the store disagree. The
    # assertion is therefore on the agreement, not on the mechanism.
    #
    # And the agreement is with what the store *gained*, not with what it holds.
    # A run over a corpus that is already indexed correctly reports "0 indexed,
    # N unchanged, 0 chunks", and asserting its `indexed` against the store's
    # total made a correct run look like the under-reporting one. So the store
    # is measured on both sides of the run and the deltas are what the card has
    # to match. On a full drive this is the first run over `small`, so the store
    # does not exist yet and the deltas are the whole corpus; on `--only 1.6`
    # after an earlier drive they are zero, and zero indexed against zero gained
    # is the same assertion.
    path = d.fixtures.small()
    before = {row["name"]: row for row in stores(d)}

    run_id, store_name = start_index(d, path)
    run = wait_for_run(d, store_name, run_id=run_id)

    was = before.get(store_name) or {"files": 0, "chunks": 0}
    now = store_named(d, store_name)
    gained_files = now["files"] - was["files"]
    gained_chunks = now["chunks"] - was["chunks"]

    if run.get("indexed") != gained_files:
        fail(
            "the run card says it indexed %s files and %s went from %d to %d — a "
            "gain of %d. A card that under-reports makes the user's own index run "
            "look like it did almost nothing."
            % (run.get("indexed"), store_name, was["files"], now["files"], gained_files)
        )
    if run.get("chunks") != gained_chunks:
        fail(
            "the run card says %s chunks and %s went from %d to %d — a gain of %d"
            % (run.get("chunks"), store_name, was["chunks"], now["chunks"], gained_chunks)
        )


@finding("1.7", "LAST WRITE is a real time for a store that has been written")
def _(d):
    store_name = indexed_fixture(d, d.fixtures.small())
    for row in stores(d):
        if row.get("unopened"):
            continue
        if row["files"] > 0 and not row.get("last_write"):
            fail(
                "%s holds %d files and reports no last write. 'Never' meant 'not "
                "during this daemon session', which is not what the column says: "
                "a store that plainly has been written must carry the time it was."
                % (row["name"], row["files"])
            )

    d.open_view("stores")
    body = view_text(d)
    if re.search(r"\bnever\b", body, re.IGNORECASE) and store_named(d, store_name)["files"] > 0:
        fail(
            "the Stores page still prints 'never' in the LAST WRITE column while "
            "a store on it holds files"
        )


@finding("1.8", "ledger and portal timestamps carry the same zone, and say which")
def _(d):
    # The ambiguity: the finding says neither surface is labelled and the two
    # disagree; it does not say which zone wins. This asserts the property the
    # finding actually complains about — that a portal time and a ledger time
    # can be correlated — by requiring both to name their offset. A fix that
    # labels both as UTC would satisfy it too; this release made both local.
    #
    # Each row carries an `at` (epoch seconds, for sorting) and a `when` that
    # is the rendered timestamp, so the assertion is on `when`: an epoch is
    # already unambiguous and asserting on it would prove nothing about what
    # either surface shows a person.
    #
    # A row to read, made rather than waited for. This drive gets its own empty
    # store home, so nothing has been retrieved when this check runs and a skip
    # would quietly retire a P1 gate.
    indexed_fixture(d, d.fixtures.small())
    d.api("/api/search?query=release%20record%20sealed%20immutable&k=8")
    # The ledger write is on the search request's own path; the read below is a
    # separate connection. One short settle beats a retry loop nobody reads.
    time.sleep(1.0)

    ledger = d.api("/api/ledger")
    rows = ledger.get("rows")
    if rows is None:
        fail(
            "/api/ledger returns no rows, so the portal cannot show the ledger "
            "at all (see finding 3.16) and its timestamps cannot be compared "
            "with the CLI's"
        )
    if not rows:
        fail(
            "a search was just run through /api/search and /api/ledger lists no "
            "rows, so nothing was recorded. The ledger is what makes a retrieval "
            "auditable; an empty one is not a timestamp problem, it is a missing "
            "record."
        )

    stamped = re.compile(r"^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2} [+-]\d{2}:\d{2}$")
    offsets = set()
    for row in rows[:20]:
        when = str(row.get("when") or "")
        if not stamped.match(when):
            fail(
                "a ledger row's `when` is %r, which is not a timestamp naming its "
                "offset. The CLI printed UTC and the portal printed local time "
                "with no marker on either, so a portal event could not be lined "
                "up with a ledger row." % when
            )
        offsets.add(when[-6:])

    # And the portal shows that string rather than a clock of its own: the
    # browser's local time was the other half of the disagreement.
    d.open_view("ledger")
    body = view_text(d)
    shown = str(rows[0]["when"])
    if shown not in body:
        fail(
            "the newest ledger row is timestamped %r and the Retrieval ledger "
            "page does not print it. The page used to render the browser's own "
            "clock, which is how the same retrieval read 13:40:32 in one place "
            "and 19:00:18 in the other." % shown
        )

    # The CLI is the other reader of the same rows, and the finding is about
    # the two disagreeing, so it is asked directly rather than assumed.
    try:
        printed = fixtures.run([fixtures.semlith_bin(), "ledger"], timeout=120)
    except fixtures.FixtureError as error:
        skip("the binary under test could not be run: %s" % error)

    clocks = re.findall(r"\b\d{2}:\d{2}:\d{2} [+-]\d{2}:\d{2}\b", printed)
    bare = re.findall(r"\b\d{2}:\d{2}:\d{2}\b", printed)
    if not bare:
        skip("`semlith ledger` printed no rows from the fleet this shell can see")
    if not clocks:
        fail(
            "`semlith ledger` prints %s and names no offset on any of them, so "
            "its rows still cannot be lined up with the portal's."
            % ", ".join(repr(t) for t in bare[:3])
        )
    printed_offsets = {clock[-6:] for clock in clocks}
    if printed_offsets != offsets:
        fail(
            "`semlith ledger` prints times at offset %s and /api/ledger's rows "
            "carry %s. One dataset, two clocks, which is the whole of this finding."
            % (", ".join(sorted(printed_offsets)), ", ".join(sorted(offsets)))
        )


# ==========================================================================
# P2 — a feature does not work
# ==========================================================================


@finding("2.1", "a folder holding a .semlith store can be adopted through the portal")
def _(d):
    folder = d.fixtures.adoptme()
    status, answer = d.api_result("/api/adopt", method="POST", body={"path": folder})
    if status != 200:
        fail(
            "adopting %s was refused with %d: %s\nThe folder holds a valid "
            "`.semlith` built by the binary under test, and the picker cannot "
            "reach a dot-directory, so the parent has to be accepted."
            % (folder, status, json.dumps(answer)[:300])
        )

    # Not `/api/stores`: the daemon opened its stores at startup and holds
    # their locks, so a store adopted now joins on its next start — which is
    # what `restart_required` in the answer says. Asserting it is in the list
    # already would be asserting the opposite of the documented behaviour.
    want("the adopt's restart_required", answer.get("restart_required"), True)
    name = answer.get("name")
    if not name:
        fail("the adopt answered 200 with no store name: %s" % json.dumps(answer)[:300])

    # So the two halves of a successful adopt are asserted directly: the
    # registry gained the store, and the folder no longer holds it.
    registry_path = os.path.join(
        os.environ.get("SEMLITH_HOME") or os.path.join(os.path.expanduser("~"), ".semlith"),
        "registry.json",
    )
    try:
        with open(registry_path, "r", encoding="utf-8") as handle:
            registry = json.load(handle)
    except (OSError, ValueError) as error:
        fail("the adopt answered 200 and %s could not be read: %s" % (registry_path, error))

    entry = (registry.get("stores") or {}).get(name)
    if entry is None:
        fail(
            "the adopt registered %r and it is not in %s. Nothing else records "
            "that the store exists, so it would not come back on the next start. "
            "Registered: %s"
            % (name, registry_path, ", ".join(sorted(registry.get("stores") or {})) or "nothing")
        )
    roots = [normalise(root) for root in entry.get("roots") or []]
    if normalise(folder) not in roots:
        fail(
            "%s was adopted from %s and its registry entry covers %s instead. The "
            "root defaults to the directory the store sat in, which is the corpus "
            "it was indexing." % (name, folder, ", ".join(roots) or "nothing")
        )

    left = os.path.join(folder, ".semlith")
    if os.path.exists(left):
        fail(
            "the adopt answered 200 and %s is still there. Adopting moves the "
            "store into the home; a copy left behind is a second store of the "
            "same corpus that nothing will ever write to again." % left
        )


@finding("2.2", "the URL panel names its own store, and a private address is refused as one")
def _(d):
    d.open_view("index")
    # The panel is behind its own button, like the other three on this page.
    d.open_index_panel("Add from a URL")
    own_store_control = d.eval(
        """
        (() => {
          const field = document.querySelector('#index-url');
          if (!field) return null;
          const panel = field.closest('.card');
          const select = panel.querySelector('select, [role=combobox], input[list]');
          if (!select) return null;
          // The label has to be the panel's own, not one borrowed from the
          // control group above: that is exactly what went wrong.
          const label = panel.querySelector(`label[for="${select.id}"]`);
          return {label: label ? label.textContent.trim() : null,
                  options: [...select.options || []].map(o => o.value)};
        })()
        """
    )
    if own_store_control is None:
        fail(
            "the 'Add from a URL' panel still has no store selector of its own. "
            "The control it reads sits in a different control group above, and "
            "the error it produces names the stores without saying where to name one."
        )
    want("the URL panel's store selector label", own_store_control["label"], "Fetch into")
    d.close_index_panel("Add from a URL")

    # With a store named, the request reaches URL validation, so the privacy
    # refusal for a private address is actually exercised from the portal.
    open_stores = [row["name"] for row in stores(d) if not row.get("unopened")]
    if not open_stores:
        skip("no store is open to fetch into")
    status, answer = d.api_result(
        "/api/add",
        method="POST",
        body={"url": d.portal_url + "/", "store": open_stores[0]},
    )
    message = json.dumps(answer).lower()
    if status == 200:
        fail("fetching a private loopback address was accepted; it must be refused")
    if "store" in message and "private" not in message and "local" not in message:
        fail(
            "fetching a private address answered %d with a store-selection error "
            "(%s). With a store named, the refusal must be the privacy one."
            % (status, json.dumps(answer)[:200])
        )


@finding("2.3", "Open in Files filters the Files page to that store")
def _(d):
    # Two stores with files, built rather than hoped for: telling a store filter
    # from no filter needs something to filter out, and this drive starts
    # against an empty store home.
    target = indexed_fixture(d, d.fixtures.small())
    indexed_fixture(d, d.fixtures.second())

    d.open_view("stores")
    d.wait_for(
        "[...document.querySelectorAll('tbody tr')].some(r =>"
        " ((r.querySelector('.name') || {}).textContent || '').trim() === %s)"
        % json.dumps(target),
        what="the %s row on the Stores page" % target,
    )
    # Opened and chosen in one tick. The Stores page rebuilds its rows from a
    # one-second poll, so a menu opened in one CDP call and clicked in the next
    # can be hanging off a row that has since been replaced — which is how this
    # check used to land on an unfiltered Files page and blame the product.
    # `rowMenu` builds its items from the row's own store at open time, so the
    # only thing that has to be atomic is open-then-click.
    opened = d.eval(
        """
        (() => {
          const rows = [...document.querySelectorAll('tbody tr')];
          // The store's own name cell — `lineCell(s.name, "name")` — not the
          // row's text, which also carries its path, its badges and its counts.
          const row = rows.find(r =>
            ((r.querySelector('.name') || {}).textContent || '').trim() === %s);
          if (!row) return 'no row for the store';
          const kebab = row.querySelector('button[aria-haspopup], button[aria-label*="action" i], td:last-child button');
          if (!kebab) return 'the row carries no actions control';
          kebab.click();
          const item = [...document.querySelectorAll('.menu-item, [role=menuitem]')]
            .find(b => (b.textContent || '').trim() === 'Open in Files');
          if (!item) return 'the row menu opened with no Open in Files item';
          item.click();
          return 'opened';
        })()
        """
        % json.dumps(target)
    )
    if opened != "opened":
        fail("could not drive the %s row's menu to Open in Files: %s" % (target, opened))

    # Both halves before anything is read: the view, and its store control
    # actually reading the store the menu was opened on. A row read while the
    # page is still showing every store is a read of the wrong page.
    chip = '.store-chips [data-store=%s][aria-pressed="true"]' % json.dumps(target)
    d.wait_for(
        "location.hash.startsWith('#files') && !!document.querySelector(%s)"
        " && document.querySelectorAll('tbody tr').length > 0" % json.dumps(chip),
        what="the Files view's store control to read %s, with its rows drawn" % target,
    )

    shown = [t for t in texts_of(d, "tbody tr") if t]
    if not shown:
        fail("Open in Files landed on an empty table")
    wrong = [t for t in shown if target not in t]
    if wrong:
        fail(
            "Open in Files from %s showed rows from other stores. It navigated "
            "to #files and applied no filter at all, which leaves no way "
            "anywhere in the portal to look at one store's files. First stray "
            "row: %s" % (target, wrong[0][:120])
        )


@finding("2.4", "with every edge-kind chip off, the graph draws no edges and says so")
def _(d):
    d.open_view("graph")
    d.wait_for("/\\d+\\s+edges/i.test(document.querySelector('#root').innerText)",
               what="the graph summary")

    def summary_edges():
        match = re.search(r"([\d,]+)\s+edges", view_text(d), re.IGNORECASE)
        if not match:
            fail("the graph page no longer prints an edge count")
        return int(match.group(1).replace(",", ""))

    base = summary_edges()
    chips = d.eval(
        "[...document.querySelectorAll('[aria-pressed]')].filter(c =>"
        " /calls|defines|imports|references|contains|aliases/i.test(c.innerText)).length"
    )
    if not chips:
        skip("this store's graph has no edge-kind chips to toggle")

    d.eval(
        """
        [...document.querySelectorAll('[aria-pressed="true"]')]
          .filter(c => /calls|defines|imports|references|contains|aliases/i.test(c.innerText))
          .forEach(c => c.click());
        """
    )
    d.eval("new Promise(done => setTimeout(() => done(true), 800))")

    off = summary_edges()
    if off != 0:
        fail(
            "with all %d edge-kind chips off the summary says %d edges (the "
            "unfiltered graph says %d). 'No filter selected' must mean no edges, "
            "not every edge, or the summary asserts a number that does not "
            "describe what is drawn." % (chips, off, base)
        )


@finding("2.5", "the projects picker can be pointed somewhere other than $HOME")
def _(d):
    monorepo = d.fixtures.monorepo()
    d.open_view("index")
    # The checklist is one of the Index page's four folded panels, so it is
    # opened here rather than assumed.
    d.open_index_panel("Projects under a folder…")

    has_up = d.eval(
        "[...document.querySelectorAll(%s)]"
        ".some(b => (b.innerText || '').trim() === 'Up')"
        % json.dumps(OPEN_PICKER + " button")
    )
    if not has_up:
        fail(
            "the projects picker has no Up control, so it cannot leave the home "
            "directory. Its sibling, Choose folders…, has full navigation."
        )

    # Navigation is asserted by doing it: walking down to the monorepo is the
    # whole of "somewhere other than $HOME", and every step of it is a click on
    # a folder name that used to render as an unclickable SPAN.
    descend_picker(d, monorepo)
    want("the folder the projects picker reached", picker_where(d), monorepo)

    # And Up goes back up, which is the control the finding is named for.
    d.click_text(OPEN_PICKER + " button", "Up")
    d.wait_for(
        "(document.querySelector(%s) || {}).innerText === %s"
        % (json.dumps(OPEN_PICKER + " .where"), json.dumps(os.path.dirname(monorepo))),
        what="Up to leave %s" % monorepo,
    )
    descend_picker(d, monorepo)

    # Having navigated, the checklist offers the two repositories under it.
    offered = d.eval(
        "[...document.querySelectorAll(%s)].map(e => e.textContent.trim())"
        % json.dumps(OPEN_PICKER + " .entry .name")
    )
    for expected in ("repo-one", "repo-two"):
        if expected not in offered:
            fail(
                "the projects checklist at %s does not offer %s: it lists %s"
                % (monorepo, expected, ", ".join(offered) or "nothing")
            )

    # And the daemon agrees about which of them are repositories, so the page
    # is not listing plain subfolders and calling them projects.
    listing = d.api("/api/projects?path=%s" % monorepo)
    if not listing.get("repositories"):
        fail(
            "/api/projects says nothing under the monorepo fixture is a "
            "repository, though the fixture builds two with git"
        )
    found = {row.get("name") for row in listing.get("projects") or []}
    for expected in ("repo-one", "repo-two"):
        if expected not in found:
            fail("the projects listing for the monorepo fixture is missing %s: saw %s"
                 % (expected, sorted(n for n in found if n)))

    # Left as it was found: this panel is exclusive with the other three.
    d.close_index_panel("Projects under a folder…")


def stop_quietly(d, store):
    """Stop a run if one is going, and say nothing when there is not.

    Tidying up after a check. A run that finished on its own is refused with a
    409 that names exactly that, which is the product being right rather than
    something for the drive to raise.
    """
    try:
        d.api("/api/index/control", method="POST", body={"store": store, "action": "stop"})
    except cdp.ProtocolError as refused:
        if "no run to stop" not in str(refused):
            raise


@finding("2.6", "finished runs can be dismissed and the active run sorts first")
def _(d):
    # A corpus nobody else indexes, so exactly one card on the page carries this
    # store and that card is this check's handle on its own run. Counting cards
    # instead does not work on a live page: the Index page keeps painting while
    # this check waits — its own second half starts another run, and so does
    # every check after it — so the total can rise past where it started while
    # the dismissed card is long gone. The assertion is on *that* card going.
    finished = indexed_fixture(d, d.fixtures.unique("solo"))
    d.open_view("index")
    mine = (
        "[...document.querySelectorAll(%s)].filter(c =>"
        " ((c.querySelector('.card-title') || {}).textContent || '').trim() === %s)"
        % (json.dumps(RUN_CARD), json.dumps(finished))
    )
    # The card's own Remove, not the "Remove all finished" button above the
    # cards and not another run's: found inside that one card, and a hidden
    # button is skipped so only the control on offer can match.
    #
    # Waited for rather than assumed. A card is built from the page's last poll,
    # which can be a second old, so a run this check has already watched finish
    # is painted as live for up to that long — with Pause and Stop where its
    # Remove will be.
    offered = (
        "%s.filter(c => [...c.querySelectorAll('button')].some(b =>"
        " !b.hidden && b.offsetParent !== null"
        " && (b.textContent || '').trim() === 'Remove'))" % mine
    )
    d.wait_for(
        "%s.length === 1" % offered,
        what="the finished run card for %s, with its Remove on offer" % finished,
    )

    clicked = d.eval(
        """
        (() => {
          const card = %s[0];
          if (!card) return 'the card went before it could be removed';
          const remove = [...card.querySelectorAll('button')].find(
            b => !b.hidden && b.offsetParent !== null
                 && (b.textContent || '').trim() === 'Remove');
          if (!remove) return 'that card offers no Remove';
          remove.click();
          return 'removed';
        })()
        """
        % offered
    )
    if clicked != "removed":
        fail("Remove on the finished run card for %s: %s" % (finished, clicked))

    d.wait_for(
        "%s.length === 0" % mine,
        timeout=30,
        what="the dismissed card for %s to go" % finished,
    )

    # And when something is actually running, it is the card at the top.
    _, busy = start_index(d, d.fixtures.bulk())
    d.open_view("index")
    # Both conditions in one wait, not two reads. The page repaints its cards
    # from the shared one-second poll, so between "the new card exists" and "the
    # list has been re-sorted around it" there is a frame in which the finished
    # card is still first — and reading once in that frame reported a sort bug
    # that was a repaint the check did not wait for. A bounded wait, because a
    # live run that never reaches the top is exactly what this finding is about.
    try:
        d.wait_for(
            "(() => { const cards = [...document.querySelectorAll(%s)];"
            " const first = cards[0];"
            " return cards.some(c => c.innerText.includes(%s))"
            " && !!first && first.innerText.includes(%s); })()"
            % (json.dumps(RUN_CARD), json.dumps(busy), json.dumps(busy)),
            timeout=30,
            what="the live run on %s to be the first card on the Index page" % busy,
        )
    except cdp.ProtocolError:
        # A run that finished while we were watching is not a failure of the
        # ordering. The fixture corpus is already indexed by the time this
        # check runs, so a second run over it can finish in under a second on a
        # fast machine and there is no live card left to be first.
        still_live = any(
            run["store"] == busy and run["status"] in ("queued", "running", "paused", "stopping")
            for run in (d.api("/api/index/runs").get("runs") or [])
        )
        first = d.eval(
            "(document.querySelector(%s) || {}).innerText || ''" % json.dumps(RUN_CARD)
        )
        stop_quietly(d, busy)
        if still_live:
            fail(
                "a run that is still going (%s) never became the first card on the "
                "page; after 30s of repaints the top card still reads %r"
                % (busy, first[:120])
            )
        skip(
            "the run over %s finished before the ordering could be observed; "
            "there was no live card to sort above the finished ones" % busy
        )
    stop_quietly(d, busy)


@finding("2.7", "lang:, path: and budget re-run the query like every other filter")
def _(d):
    d.open_view("search")
    d.type(SEARCH_BOX, "release record sealed immutable")
    d.press("Enter")
    d.wait_for("document.querySelectorAll(%s).length > 0" % json.dumps(RESULT_CARD),
               what="search results")
    before = texts_of(d, RESULT_CARD)

    # `labelled("search-path", …)` gives the `path:` dial's field its id.
    if not exists(d, "#search-path"):
        fail("no `path:` filter field was found on the Search page")

    d.type("#search-path", "**/*.md")
    # Deliberately no Enter in the main query box: that is exactly the
    # workaround the finding says a user should not need. The filters debounce
    # by 250ms and then re-run the query, so this waits for the answer rather
    # than for a fixed number of milliseconds.
    try:
        d.wait_for(
            "[...document.querySelectorAll(%s)].map(e => (e.innerText || '').trim()).join('\\n') !== %s"
            % (json.dumps(RESULT_CARD), json.dumps("\n".join(before))),
            timeout=10,
            what="the `path:` filter to re-run the query",
        )
    except cdp.ProtocolError:
        # Let the assertion below say what went wrong, in the finding's words.
        pass
    after = texts_of(d, RESULT_CARD)

    if after == before:
        fail(
            "setting the `path:` filter left the results exactly as they were. "
            "The store chips, the k stepper and the prefer segments all re-run "
            "immediately; the user is looking at results that contradict the "
            "visible filter state, with nothing to say so."
        )
    stray = [row for row in after if ".md" not in row.lower()]
    if stray:
        fail("the results after `path: **/*.md` still include %r" % stray[0][:120])


@finding("2.8", "the graph's scope field says how it is applied, and both ways work")
def _(d):
    # The finding's complaint is that a text field silently waiting for Enter
    # reads as broken beside click-to-apply chips, and it allowed either of two
    # answers: apply as you type, or give the field a visible submit affordance
    # that says Enter also works. The product took the second, so that is what
    # is asserted here — the button, the hint that names the key, and both
    # routes actually applying a scope. A release that later switches to
    # apply-as-you-type must rewrite this check, not delete it.
    #
    # From 0.27.0 the hint is read rather than seen. The sentence sat inside the
    # field's border and pushed the button off its end, so it is `.sr-only` now:
    # the button is the affordance for anyone looking at the field, and the
    # description is still there for anyone who is not. Both halves of the
    # finding still hold; only which sense they reach changed.
    d.open_view("graph")
    d.wait_for("/\\d+\\s+symbols/i.test(document.querySelector('.graph-count').innerText)",
               what="the graph summary")

    if not exists(d, "#graph-scope"):
        fail("no 'Scope to a path, or find a symbol' field was found on the Graph page")

    affordance = d.eval(
        """
        (() => {
          const field = document.querySelector('#graph-scope');
          const scope = field.closest('.graph-scope');
          if (!scope) return null;
          const button = [...scope.querySelectorAll('button')]
            .find(b => (b.innerText || '').trim().length > 0);
          const described = field.getAttribute('aria-describedby');
          const hint = described && document.getElementById(described);
          // `textContent`, not `innerText`: from 0.27.0 the hint is
          // screen-reader-only, and `innerText` is what is rendered — which is
          // nothing, by design. A screen reader reads the text either way, and
          // the text is what this finding is about.
          return {button: button ? (button.innerText || '').trim() : null,
                  describedby: described,
                  hint: hint ? (hint.textContent || '').trim() : null};
        })()
        """
    )
    if affordance is None:
        fail("the graph's scope field is no longer in a .graph-scope group")
    if not affordance["button"]:
        fail(
            "the graph's scope field has no button beside it. Every other filter "
            "on the page is a click-to-apply chip, so a text field that answers "
            "only Enter reads as broken — it needs either a submit affordance or "
            "apply-as-you-type, and this release chose the affordance."
        )
    if not affordance["hint"] or "enter" not in affordance["hint"].lower():
        fail(
            "the scope field's button is there and nothing names the key that "
            "also applies it. Its aria-describedby is %r and reads %r; it should "
            "say Enter applies it."
            % (affordance["describedby"], affordance["hint"])
        )

    # Two symbols the graph actually holds, so "applied" can be asserted on the
    # rail naming the symbol that was asked for rather than on the page merely
    # having changed.
    def selected():
        return d.eval(
            "((document.querySelector('.graph-selected .sym') || {}).innerText || '').trim()"
        )

    overview = d.api("/api/graph?limit=25")
    plain = [
        node["name"]
        for node in overview.get("nodes") or []
        # A name carrying a dot or a slash is read as a path to scope to rather
        # than as a symbol to centre on, which is a different code path.
        if node.get("name") and "." not in node["name"] and "/" not in node["name"]
    ]
    landed = selected()
    targets = [name for name in plain if name != landed][:2]
    if len(targets) < 2:
        skip("this graph holds fewer than two symbols that can be scoped to by name")

    # The button applies it.
    d.type("#graph-scope", targets[0])
    d.click_text(".graph-scope button", affordance["button"])
    d.wait_for(
        "((document.querySelector('.graph-selected .sym') || {}).innerText || '').trim() === %s"
        % json.dumps(targets[0]),
        what="the %r button to apply the scope %r" % (affordance["button"], targets[0]),
    )

    # And so does Enter, which is what the hint promises.
    d.type("#graph-scope", targets[1])
    d.press("Enter")
    d.wait_for(
        "((document.querySelector('.graph-selected .sym') || {}).innerText || '').trim() === %s"
        % json.dumps(targets[1]),
        what="Enter to apply the scope %r, as the hint says it does" % targets[1],
    )


@finding("2.9", "the PWA manifest's icons resolve, and a page load logs no errors")
def _(d):
    status, manifest = d.api_result("/icons/site.webmanifest")
    if status != 200:
        fail("/icons/site.webmanifest answered %d" % status)
    icons = manifest.get("icons") or []
    if not icons:
        fail("the manifest declares no icons")
    for icon in icons:
        src = icon.get("src", "")
        # Resolved the way a browser resolves it: relative to the manifest's
        # own URL, which is what put the second `icons/` in the path.
        resolved = src if src.startswith("/") else "/icons/" + src.lstrip("./")
        code = d.status(resolved)
        if code != 200:
            fail(
                "the manifest's icon %r resolves to %s, which answers %d. The "
                "file is one level up; the fix is to drop the `icons/` prefix or "
                "make the srcs absolute." % (src, resolved, code)
            )

    d.clear_console()
    d.open_view("stores", fresh=True)
    errors = d.console_errors()
    if errors:
        fail("a page load logged %d console errors:\n  %s" % (len(errors), "\n  ".join(errors[:5])))


@finding("2.10", "an unknown path is 404, not 401")
def _(d):
    # The ambiguity: the daemon checks the token before it routes, so an
    # unknown path is indistinguishable from a guarded one to an anonymous
    # caller. Asserted here with the token held, which is the case the finding
    # reproduced and the one that misleads someone debugging a typo'd asset.
    for path in ("/nope.png", "/icons/nope.png", "/api/nope", "/icons/"):
        code = d.status(path)
        if code != 404:
            fail(
                "%s answered %d with a valid token. A 401 on a path that does "
                "not exist sends anyone debugging a typo'd asset looking at the "
                "token instead of the path." % (path, code)
            )
    for path in ("/style.css", "/app.js"):
        want("%s is still served" % path, d.status(path), 200)


# ==========================================================================
# P3 — inconsistency and contradiction
# ==========================================================================


@finding("3.1", "a registry entry with no directory is badged, and not offered as a target")
def _(d):
    store_name = indexed_fixture(d, d.fixtures.doomed())
    d.fixtures.remove_doomed()
    # The daemon notices on its next read of the route; one poll is enough.
    d.api("/api/stores")
    time.sleep(1.0)

    row = store_named(d, store_name)
    roots = row.get("roots") or []
    if roots and all(r.get("present") for r in roots):
        skip("the daemon has not noticed the removed root yet")

    d.open_view("stores")
    # Waited for, not read once. The page is live: it polls every second and
    # redraws what moved, so a row read in the same breath as the navigation is
    # a row drawn from the answer before the root was removed.
    read_row = (
        "(() => { const r = [...document.querySelectorAll('tbody tr')]"
        ".find(r => r.innerText.includes(%s)); return r ? r.innerText : null; })()"
        % json.dumps(store_name)
    )
    try:
        d.wait_for(
            "(() => { const r = [...document.querySelectorAll('tbody tr')]"
            ".find(r => r.innerText.includes(%s));"
            " return !!r && /missing|gone|not there|unavailable/i.test(r.innerText); })()"
            % json.dumps(store_name),
            timeout=15,
            what="the Stores row for %s to say its root is gone" % store_name,
        )
    except cdp.ProtocolError:
        # Fall through to the assertion below, which says it in the finding's
        # own words and quotes what the row actually read.
        pass
    row_text = d.eval(read_row)
    if row_text is None:
        fail("the Stores page no longer lists %s at all" % store_name)
    if not re.search(r"missing|gone|not there|unavailable", row_text, re.IGNORECASE):
        fail(
            "%s has no root directory and its Stores row shows no badge saying "
            "so: %r. The API already returns the data a badge needs — `roots` is "
            "empty and `present` is false — and nothing uses it."
            % (store_name, row_text.replace("\n", " · ")[:160])
        )

    d.open_view("index")
    offered = d.eval(
        "[...document.querySelectorAll('option')].map(o => (o.innerText||'').trim())"
    )
    if any(store_name in option for option in offered):
        fail(
            "the Index page still offers 'add to %s' for a store whose directory "
            "does not exist. That is the same mechanism that produced finding 1.1."
            % store_name
        )


@finding("3.2", "the machine limits panel does not contradict itself")
def _(d):
    d.open_view("index")
    # The three numbers live behind the "Machine limits" button, folded away
    # because they are read once and changed rarely.
    d.open_index_panel("Machine limits")
    panel = limits_panel(d)

    if re.search(r"\b0\s*GiB is free", panel, re.IGNORECASE):
        fail(
            "the panel says '0 GiB is free' — an integer truncation — three "
            "lines below a figure in MiB that is not zero:\n%s" % panel[:400]
        )

    # A reserve larger than what is free is a negative, and the copy concluded
    # "allows 1" anyway. Whatever the numbers are, when a floor is applied the
    # sentence has to admit it.
    reserve = re.search(r"([\d.]+)\s*GiB free minus a ([\d.]+)\s*GiB reserve", panel, re.IGNORECASE)
    if reserve:
        free, held = float(reserve.group(1)), float(reserve.group(2))
        if free < held and not re.search(r"floor|at least|minimum", panel, re.IGNORECASE):
            fail(
                "the panel subtracts a %.1f GiB reserve from %.1f GiB free and "
                "concludes it 'allows 1' without saying a floor was applied:\n%s"
                % (held, free, panel[:400])
            )

    if re.search(r"\bMiB per store\b", panel) and re.search(r"\bMB\b", panel):
        fail(
            "the field is labelled MiB per store and its help text says MB. One "
            "card, two units for one number:\n%s" % panel[:400]
        )
    d.close_index_panel("Machine limits")


@finding("3.3", "'threads each' recomputes when 'runs at once' changes")
def _(d):
    d.open_view("index")
    d.open_index_panel("Machine limits")
    if not exists(d, RUNS_AT_ONCE):
        fail("no 'runs at once' field was found in the machine limits panel")
    if d.eval("document.querySelector(%s).disabled" % json.dumps(RUNS_AT_ONCE)):
        skip("this machine's runs-at-once is set by the environment, so the page cannot change it")

    # `settingField` saves on `change`, which `d.type` dispatches; the panel is
    # then repainted from the daemon's answer.
    d.type(RUNS_AT_ONCE, "3")
    try:
        d.wait_for(
            "/\\b3 runs\\b/.test(document.querySelector(%s).closest('.card').innerText)"
            % json.dumps(RUNS_AT_ONCE),
            timeout=15,
            what="the limits panel to be repainted for 3 runs at once",
        )
    except cdp.ProtocolError:
        # Let the assertions below say which derivation went stale.
        pass

    # Each field's derivation is the `.note` under it, not a paragraph.
    help_text = d.eval(
        """
        (() => {
          const all = [...document.querySelectorAll('.setting .note, .note')];
          const el = all.find(e => /embedding threads/i.test(e.innerText || ''));
          return el ? el.innerText : null;
        })()
        """
    )
    if help_text is None:
        fail("no 'threads each' help text was found")
    if re.search(r"\bbetween 1 run\b", help_text, re.IGNORECASE):
        fail(
            "'runs at once' was set to 3 and the 'threads each' help still reads "
            "%r. The sibling field's derivation is stale." % help_text
        )
    if not re.search(r"\b3 runs?\b", help_text):
        fail("'threads each' help does not mention the 3 runs now configured: %r" % help_text)
    # Left open for 3.4, which reads the same panel for the saved value this
    # check has just written.


@finding("3.4", "a value the daemon calls saved, the portal calls saved too")
def _(d):
    d.open_view("index")
    d.open_index_panel("Machine limits")
    panel = limits_panel(d)
    # 3.3 has just written a value, so this run of the drive has a saved one.
    if re.search(r"\bderived\b", panel) and not re.search(r"\bsaved\b", panel):
        fail(
            "the panel calls the runs-at-once value 'derived' when settings.json "
            "holds a saved one. The daemon logs it as '(saved)', and the two only "
            "diverge when the saved value equals the derived one — exactly when a "
            "user is trying to work out whether their setting took effect:\n%s"
            % panel[:300]
        )
    d.close_index_panel("Machine limits")


@finding("3.5", "the graph's CALLERS list holds only call edges")
def _(d):
    d.open_view("graph")
    # The rail fills from `/api/neighbors` after the graph itself has drawn, so
    # it is waited for by its own markup — `ends(title, …)` builds one
    # `.rail-group` per list, headed by an `h3`.
    # Scoped to `.graph-rail`, because the shell's own navigation is built from
    # `.rail-group` divs too and a bare `.rail-group` matches four of them
    # before it reaches the graph's.
    d.wait_for(
        "!!document.querySelector('.graph-rail .rail-group h3')",
        timeout=30,
        what="the graph rail's edge lists for the symbol it opened on",
    )
    # The finding allowed either of two answers: filter the list to `calls`, or
    # rename it to what it holds. The product took the rename — the heading is
    # now "Incoming" — so the property asserted is the one both answers share:
    # a list whose heading promises calls holds only calls. Each row carries its
    # own kind in `el("span", {class: "via", text: end.kind})`, so the kinds are
    # read rather than pattern-matched out of the row's prose.
    rail = d.eval(
        """
        (() => {
          const groups = [...document.querySelectorAll('.graph-rail .rail-group')];
          if (!groups.length) return null;
          return groups.map(group => ({
            head: ((group.querySelector('h3') || {}).innerText || '').trim(),
            kinds: [...group.querySelectorAll('li')]
              .map(row => ((row.querySelector('.via') || {}).innerText || '').trim())
              .filter(Boolean),
          }));
        })()
        """
    )
    if not rail:
        fail(
            "the graph rail lists no edge groups at all for the symbol the page "
            "opened on. `ends(…)` builds one `.rail-group` with an `h3` per list, "
            "even when the list is empty."
        )
    incoming = [g for g in rail if re.match(r"^(incoming|callers)\b", g["head"], re.IGNORECASE)]
    if not incoming:
        fail(
            "the graph rail has no list of the edges that point at the selected "
            "symbol. Its groups are headed %s."
            % ", ".join(repr(g["head"]) for g in rail)
        )
    for group in incoming:
        if not group["kinds"]:
            continue
        promises_calls = re.match(r"^callers\b", group["head"], re.IGNORECASE)
        wrong = sorted({k for k in group["kinds"] if k.lower() != "calls"})
        if promises_calls and wrong:
            fail(
                "the rail's %r list holds %s, which are not calls. The section is "
                "really 'incoming edges'; either it says so — as it now does — or "
                "it filters to calls." % (group["head"], ", ".join(wrong))
            )


@finding("3.6", "the graph opens on something legible and offers fit and zoom")
def _(d):
    d.open_view("graph")
    d.wait_for("/\\d+\\s+symbols/i.test(document.querySelector('#root').innerText)",
               what="the graph summary")

    controls = texts_of(d, "button")
    if not any(re.search(r"\b(fit|reset|zoom|\+|−|-)\b", c, re.IGNORECASE) for c in controls):
        fail(
            "the graph offers only %s. With no zoom, fit or reset there is no way "
            "to read a view whose labels have collided."
            % ", ".join(c for c in controls if c)[:160]
        )

    # `new` is the worst possible default hub in a Rust codebase: every type
    # has one, so the default view radiates every edge in the store from it.
    # Label collision itself is drawn on a canvas and cannot be measured from
    # the DOM; the default selection can.
    selected = d.eval(
        "(document.querySelector('[aria-current=\"true\"], .selected, [aria-selected=\"true\"]')"
        " || {}).innerText || ''"
    ).strip()
    if selected.lower() == "new":
        fail(
            "the graph still opens on `new`, the single worst hub in a Rust "
            "codebase. Scoped to a real symbol the same view is legible; it is "
            "the default that is unreadable."
        )


@finding("3.7", "scoping the graph cannot raise the edge count")
def _(d):
    # Read from `/api/graph`, not from the page's `.graph-count`.
    #
    # The number the page prints is `canvas.counts()` — whatever survived the
    # edge-kind chips and the store chips that whichever check ran before this
    # one left switched on — and the page does not open on the whole graph
    # anyway: `graphView()` lands on `focus(busiest.name)`, a neighbourhood. So
    # the unscoped reading came back as two symbols and every comparison after
    # it was against the wrong denominator. Both numbers now come from one
    # route, with the same `limit` and the same (absent) store filter, so the
    # only difference between the two answers is the scope. Nothing on the page
    # is touched, so its chips are left exactly as they were found.
    LIMIT = 200

    def counts(data):
        return (len(data.get("nodes") or []), len(data.get("edges") or []))

    whole = d.api("/api/graph?limit=%d" % LIMIT)
    base_symbols, base_edges = counts(whole)
    if not base_symbols:
        skip("this machine's stores hold no extracted symbols to scope")

    # The same value the scope box would send: `applyScope()` reads anything
    # without a dot or a slash as a symbol to centre on, and posts it as `name`.
    target = next(
        (
            node["name"]
            for node in whole["nodes"]
            if node.get("name") and "." not in node["name"] and "/" not in node["name"]
        ),
        None,
    )
    if not target:
        skip("no symbol in this graph can be scoped to by name")

    scoped_data = d.api("/api/graph?limit=%d&name=%s" % (LIMIT, urllib.parse.quote(target)))
    scoped_symbols, scoped_edges = counts(scoped_data)

    # The drawn set is capped — a force layout is a hairball past a few dozen
    # nodes — so neither answer is "the whole graph", and the summary line now
    # says so ("32 of 4,092 symbols"). What must hold is that no edge is drawn
    # twice: the original defect was every open store being asked about every
    # drawn name, so an edge two stores both knew was counted once per store
    # and a twenty-node scope reported 836 edges where the page's own unscoped
    # view reported 623.
    def duplicates(data):
        seen, twice = set(), []
        for edge in data.get("edges") or []:
            key = (edge.get("from"), edge.get("to"), edge.get("kind"))
            if key in seen:
                twice.append(key)
            seen.add(key)
        return twice

    for label, data in (("the unscoped graph", whole), ("the scoped graph", scoped_data)):
        twice = duplicates(data)
        if twice:
            fail(
                "%s draws %d edge(s) twice, so its edge count is not a count of "
                "what is on the canvas: %s"
                % (label, len(twice), twice[:3])
            )

    if scoped_symbols > base_symbols:
        fail("a scoped graph has %d symbols, more than the whole graph's %d"
             % (scoped_symbols, base_symbols))
    if scoped_edges > base_edges:
        fail(
            "scoping the graph raised the edge count from %d to %d over %d "
            "symbols. A subgraph cannot hold more edges than the graph it came "
            "from." % (base_edges, scoped_edges, scoped_symbols)
        )
    pairs = scoped_symbols * (scoped_symbols - 1)
    if scoped_edges > pairs:
        fail(
            "the scoped graph reports %d edges over %d symbols, more than the %d "
            "ordered pairs available" % (scoped_edges, scoped_symbols, pairs)
        )


@finding("3.8", "Doctor gives no fix command for a client that is not installed")
def _(d):
    rows = d.api("/api/doctor").get("clients") or d.api("/api/doctor").get("rows") or []
    if not rows:
        skip("the doctor route lists no clients on this machine")
    for row in rows:
        state = str(row.get("state") or row.get("status") or "").lower()
        fix = (row.get("fix") or row.get("command") or "").strip()
        if "not installed" in state and fix and fix != "—":
            fail(
                "%s is reported as '%s' and still handed %r. Every other "
                "not-installed row gets an em dash, and the same client then "
                "shows up in the Agents dry run as 'will be created'."
                % (row.get("name"), state, fix)
            )


@finding("3.9", "one Doctor state has one fix command")
def _(d):
    rows = d.api("/api/doctor").get("clients") or []
    if not rows:
        skip("the doctor route lists no clients on this machine")

    by_state = {}
    for row in rows:
        fix = str(row.get("repair") or "").strip()
        if not fix or fix == "—":
            continue
        by_state.setdefault(doctor_state(row), {}).setdefault(fix, []).append(row.get("name"))

    for state, fixes in by_state.items():
        if len(fixes) < 2:
            continue
        # Two commands under one state are allowed when the cell says why. The
        # product's own answer is a trailing `# <client> is registered by writing
        # its config file, which \`semlith setup\` alone does not do`, which the
        # `copyField` renders with the command — so a remedy that carries an
        # explanation of its own is not an unexplained second remedy.
        unexplained = {fix: names for fix, names in fixes.items() if "#" not in fix}
        if len(unexplained) < 2:
            continue
        listed = "; ".join(
            "%s → %r" % (", ".join(names), fix) for fix, names in unexplained.items()
        )
        fail(
            "the state %r is given %d different remedies with nothing to "
            "explain the difference: %s" % (state, len(unexplained), listed)
        )


@finding("3.10", "the recommended Claude Code HTTP command carries --scope user")
def _(d):
    d.open_view("agents")
    body = view_text(d)
    command = None
    for line in body.splitlines():
        if "claude mcp add" in line and "--transport http" in line:
            command = line.strip()
            break
    if command is None:
        fail("no `claude mcp add --transport http` command was found on the Agents page")
    if "--scope user" not in command:
        fail(
            "the copyable HTTP command omits `--scope user`, which the paragraph "
            "directly above it says is 'what makes one registration cover every "
            "directory'. Copy it and semlith is registered for one directory "
            "only. Saw: %s" % command
        )


@finding("3.11", "the Agents page renders no literal backticks")
def _(d):
    d.open_view("agents")
    body = view_text(d)
    stray = [line.strip() for line in body.splitlines() if "`" in line]
    if stray:
        fail(
            "backticks are printed as characters on the Agents page, while every "
            "other inline code reference on the same page is styled:\n  %s"
            % "\n  ".join(stray[:3])
        )


@finding("3.12", "stopping the MCP endpoint asks first")
def _(d):
    d.open_view("agents")
    before = d.api("/api/agents")
    d.click_text("button", "Stop")
    d.eval("new Promise(done => setTimeout(() => done(true), 600))")

    asked = d.eval(
        "!!document.querySelector('dialog[open], [role=alertdialog], [role=dialog]')"
    )
    if not asked:
        fail(
            "Stop closed the MCP endpoint with no dialog. Delete store, Forget "
            "file, Forget selected and Stop run all confirm; the one action that "
            "severs every connected agent does not."
        )
    after = d.api("/api/agents")
    if json.dumps(after.get("endpoint")) != json.dumps(before.get("endpoint")):
        fail("the endpoint changed state before the confirmation was answered")
    d.press("Escape")


@finding("3.13", "the dry run is the primary button, writing files is not")
def _(d):
    d.open_view("agents")
    classes = d.eval(
        """
        (() => {
          const out = {};
          for (const b of document.querySelectorAll('button')) {
            const t = (b.innerText || '').trim().toLowerCase();
            if (t.startsWith('write these files')) out.write = b.className;
            if (t.startsWith('show what would be written')) out.preview = b.className;
          }
          return out;
        })()
        """
    )
    if "write" not in classes or "preview" not in classes:
        fail("the Agents page no longer carries both the write and the preview buttons")
    if "secondary" in classes["write"]:
        return
    if "secondary" not in classes["preview"] and "primary" in classes["write"]:
        fail(
            "'Write these files' is still the primary button (%r) and the safe "
            "preview is %r. The visual hierarchy is inverted relative to the "
            "risk: the primary writes MCP configuration into ten real config "
            "files across the machine." % (classes["write"], classes["preview"])
        )


@finding("3.14", "the About page's model notes describe the models they sit beside")
def _(d):
    models = d.api("/api/models")
    rows = models.get("models") if isinstance(models, dict) else models
    by_name = {row.get("name") or row.get("model"): row for row in rows}

    def note_of(name):
        row = by_name.get(name)
        if row is None:
            skip("this build no longer lists the model %s" % name)
        return (row.get("note") or row.get("description") or "").lower()

    for name in ("BGEBaseENV15Q", "GTEBaseENV15", "GTEBaseENV15Q"):
        note = note_of(name)
        if "large" in note:
            fail("%s is a base model and its note calls it large: %r" % (name, note))
    for name in ("GTEBaseENV15", "GTEBaseENV15Q"):
        note = note_of(name)
        if "multilingual" in note:
            fail("%s is the English GTE model and its note calls it multilingual: %r"
                 % (name, note))

    small = note_of("BGESmallENV15")
    if "default" in small:
        fail(
            "BGESmallENV15's note calls it the default English model. semlith's "
            "default is granite, and these notes are presented in semlith's UI as "
            "semlith's own statements about models the user is invited to choose "
            "between: %r" % small
        )


@finding("3.15", "the model actually in use carries a size")
def _(d):
    # The ambiguity: the finding complains that 43 of 48 rows are empty and
    # that sorting by SIZE hides the five that are not. It does not say every
    # model must gain a size — upstream may not publish one. The two things it
    # does imply are asserted: the model semlith is running has a size, and
    # the sort puts the rows that have one where they can be seen.
    about = d.api("/api/about")
    in_use = about.get("model") or (about.get("embedding") or {}).get("model")
    if not in_use:
        # `/api/about` describes the binary; the model belongs to a store, and
        # every store's row names the one it was built with.
        in_use = next(
            (s["model"] for s in (d.api("/api/stores").get("stores") or []) if s.get("model")),
            None,
        )
    if not in_use:
        skip("no open store names the model it was built with")

    models = d.api("/api/models")
    rows = models.get("models") if isinstance(models, dict) else models
    match = next(
        (r for r in rows if (r.get("name") or r.get("model")) == in_use), None
    )
    if match is None:
        fail("the model in use, %s, is not in the models table at all" % in_use)
    if not match.get("size") and not match.get("bytes"):
        fail(
            "the model actually in use (%s), which is downloaded and which the "
            "Privacy page reports as cached, shows no SIZE" % in_use
        )

    # The second half of this check drove the About page's models table and its
    # SIZE sort. 0.27.0 removed that table: forty-eight rows of a catalogue of
    # which any machine has fetched one, on a page the v4 design gives seven
    # facts and a language table. What the finding was actually about survives
    # above — the model in use has a size, and `/api/models` still says so —
    # and 7.9 asserts the table is gone and both routes still answer. There is
    # no sort control left to mis-sort.


@finding("3.16", "the Retrieval ledger page shows the ledger")
def _(d):
    d.api("/api/search?query=release%20record%20sealed%20immutable&k=8")
    time.sleep(1.0)
    d.open_view("ledger")
    d.eval("new Promise(done => setTimeout(() => done(true), 1000))")

    rows = d.eval("document.querySelectorAll('table tbody tr').length")
    if not rows:
        fail(
            "the Retrieval ledger page has aggregate tiles, a by-client "
            "breakdown and three snippets telling you to run `semlith ledger "
            "--last 20` in a terminal, and zero rows. The one thing described as "
            "'a debugging trail' is the one thing the page will not show. It also "
            "breaks the portal-parity rule: the CLI prints rows, the portal does not."
        )


@finding("3.17", "the LINES tile's caption describes lines")
def _(d):
    d.open_view("stores")
    caption = d.eval(
        """
        (() => {
          const tiles = [...document.querySelectorAll('.tile, .stat, figure, li')];
          const tile = tiles.find(t => /^\\s*LINES\\b/i.test(t.innerText || ''));
          return tile ? tile.innerText : null;
        })()
        """
    )
    if caption is None:
        fail("no LINES tile was found on the Stores page")
    if re.search(r"readers? in use", caption, re.IGNORECASE):
        fail(
            "the LINES tile's caption is 'readers in use' — a fact about format "
            "handlers, not about lines. Every other tile's caption describes its "
            "own number: '63 formats' under FILES, '384-dimension vectors' under "
            "CHUNKS, 'int8 quantised' under ON DISK. Saw: %r"
            % caption.replace("\n", " · ")
        )


@finding("3.18", "the LAST WRITE column holds one vocabulary")
def _(d):
    d.open_view("stores")
    values = d.eval(
        """
        (() => {
          const table = document.querySelector('table');
          if (!table) return null;
          const heads = [...table.querySelectorAll('th')].map(h => (h.innerText||'').trim().toLowerCase());
          const at = heads.findIndex(h => h.startsWith('last write'));
          if (at < 0) return null;
          return [...table.querySelectorAll('tbody tr')]
            .map(r => ((r.children[at] || {}).innerText || '').trim().toLowerCase())
            .filter(Boolean);
        })()
        """
    )
    if values is None:
        fail("no LAST WRITE column was found on the Stores page")
    if "not opened" in values:
        fail(
            "'not opened' appears in the LAST WRITE column. It is a fact about "
            "whether the daemon holds the store, not a last write, and it rendered "
            "as an amber pill beside grey ones — one column, two vocabularies, two "
            "visual treatments."
        )
    phrases = {v for v in values if not re.search(r"\d", v)}
    if len(phrases) > 1:
        fail("the LAST WRITE column uses %d different phrases for the absence of a "
             "write: %s" % (len(phrases), sorted(phrases)))


@finding("3.19", "the Files header's store count respects the filter")
def _(d):
    page = d.api("/api/files?path=**/*.no-such-extension")
    want("a filter that matches nothing returns no files", page["total"], 0)
    want("a filter that matches nothing spans no formats", page["formats"], 0)
    if page["stores"] != 0:
        fail(
            "a filter matching nothing reports '%d stores' beside '0 files · 0 "
            "formats'. Two of the three numbers respond to the filter; the third "
            "describes the daemon." % page["stores"]
        )

    d.open_view("files")
    path_field = d.eval(
        """
        (() => {
          const el = [...document.querySelectorAll('input')]
            .find(i => /path|filter|glob/i.test(i.placeholder || i.name || i.id || ''));
          return el ? (el.id ? '#' + el.id : ('input[placeholder="' + el.placeholder + '"]')) : null;
        })()
        """
    )
    if path_field:
        d.type(path_field, "**/*.no-such-extension")
        d.press("Enter")
        d.eval("new Promise(done => setTimeout(() => done(true), 1200))")
        if re.search(r"Forget drops the file's chunks", view_text(d)):
            fail(
                "the Forget footnote is still visible with no rows and no Forget "
                "buttons on the page"
            )


@finding("3.20", "the empty state after a Forget blames the corpus, not the filter")
def _(d):
    store_name = indexed_fixture(d, d.fixtures.small())
    rows = all_files(d, params="&store=%s" % store_name)
    if not rows:
        skip("the fixture store holds no files to forget")
    target = rows[0]["path"]

    d.open_view("files")
    path_field = d.eval(
        """
        (() => {
          const el = [...document.querySelectorAll('input')]
            .find(i => /path|filter|glob/i.test(i.placeholder || i.name || i.id || ''));
          return el ? (el.id ? '#' + el.id : ('input[placeholder="' + el.placeholder + '"]')) : null;
        })()
        """
    )
    if not path_field:
        fail("no path filter field was found on the Files page")
    d.type(path_field, "**/" + target.replace("\\", "/").rsplit("/", 1)[-1])
    d.press("Enter")
    d.wait_for("document.querySelectorAll('tbody tr').length > 0", what="the filtered row")

    d.api("/api/forget", method="POST", body={"store": store_name, "path": target})
    d.press("Enter")
    d.eval("new Promise(done => setTimeout(() => done(true), 1200))")

    body = view_text(d)
    if re.search(r"the filter, not the corpus", body, re.IGNORECASE):
        fail(
            "after forgetting the only row matching the filter, the table says "
            "'The filter, not the corpus — clear it and look again.' The corpus "
            "is exactly why there is nothing there."
        )


@finding("3.21", "a completed Forget says something")
def _(d):
    store_name = indexed_fixture(d, d.fixtures.small())
    rows = all_files(d, params="&store=%s" % store_name)
    if not rows:
        skip("the fixture store holds no files to forget")

    d.open_view("files")
    d.wait_for("document.querySelectorAll('tbody tr').length > 0", what="the files table")
    clicked = d.eval(
        """
        (() => {
          const rows = [...document.querySelectorAll('tbody tr')];
          const row = rows.find(r => r.innerText.includes(%s));
          if (!row) return false;
          const button = [...row.querySelectorAll('button')]
            .find(b => /forget/i.test(b.innerText || ''));
          if (!button) return false;
          button.click();
          return true;
        })()
        """
        % json.dumps(store_name)
    )
    if not clicked:
        skip("no Forget button was reachable for the fixture store's rows")
    # Confirm, because Forget is one of the actions that asks.
    d.eval(
        "[...document.querySelectorAll('dialog button, [role=dialog] button')]"
        ".filter(b => /forget|confirm|yes/i.test(b.innerText || '')).forEach(b => b.click())"
    )
    d.eval("new Promise(done => setTimeout(() => done(true), 1000))")

    announced = d.eval(
        """
        [...document.querySelectorAll('.toast, [role=status], [aria-live]')]
          .map(el => (el.innerText || '').trim()).filter(Boolean)
        """
    )
    if not announced:
        fail(
            "the row disappeared and nothing said so: no toast, no [role=status], "
            "no [aria-live] region with any text in it. The operation is correct; "
            "it is just silent."
        )


@finding("3.22", "the Privacy Rules badge agrees with the Privacy scan")
def _(d):
    d.open_view("privacy")
    body = view_text(d)
    if re.search(r"\bScan\s*\n?\s*Scan\b", body):
        fail("the Privacy page renders 'Scan' immediately followed by 'Scan'")

    scan = d.api("/api/privacy/scan")
    refused = scan.get("files") or scan.get("refused") or []
    if not refused:
        # The badge and the scan can only be caught disagreeing on a machine
        # whose stores are holding something today's rules refuse, and staging
        # one is not within a check's reach: the daemon holds the write lock,
        # the routes are held to the deny-list, and a store adopted now joins
        # on the next start. The copy assertion above this line — the "Scan
        # Scan" doubling — runs either way.
        skip(
            "no open store is holding a file today's rules would refuse, so "
            "there is no disagreement for the badge to have with the scan"
        )

    if re.search(r"rules\s*[—-]\s*all holding", body, re.IGNORECASE):
        fail(
            "the Rules badge reads 'all holding' while the page's own Scan finds "
            "%d file(s) that would be refused today. The rules are forward-looking "
            "and the stored data is historical, which is a fair distinction — but "
            "the page states it as a green badge and contradicts it a screen "
            "further down." % len(refused)
        )


@finding("3.23", "the theme toggle can return to following the system")
def _(d):
    d.open_view("stores", fresh=True)
    d.eval("try { localStorage.removeItem('semlith-theme'); } catch (e) {}")
    d.open_view("stores", fresh=True)

    start = d.eval("document.documentElement.getAttribute('data-theme')")
    if start is not None:
        fail("with no stored preference the page should follow the OS, and it set "
             "data-theme=%r" % start)

    # `paintThemeButton()` labels the control with where it goes, not where you
    # are: "Switch to light", "Switch to dark", "Follow the system theme". Only
    # the third contains the word "theme", and it is never the label in the
    # state this check starts from — so a selector matching on "theme" found
    # nothing and reported the toggle as missing. All three labels, by name.
    toggle = (
        "button[aria-label='Follow the system theme'],"
        " button[aria-label='Switch to light'],"
        " button[aria-label='Switch to dark']"
    )
    if not exists(d, toggle):
        fail("no theme toggle was found on the page")

    seen = [start]
    for _ in range(4):
        d.click(toggle)
        d.eval("new Promise(done => setTimeout(() => done(true), 300))")
        seen.append(d.eval("document.documentElement.getAttribute('data-theme')"))
        if seen[-1] is None and len(seen) > 2:
            return
    fail(
        "the theme toggle cycled through %r and never came back to following the "
        "system. The first click writes a preference to localStorage and from "
        "then on it is a two-state light/dark toggle with no way back without "
        "clearing site data." % seen
    )


@finding("3.24", "the search box has one name, and the rail names its own journey")
def _(d):
    d.open_view("search")
    top_bar = d.eval(
        """
        (() => {
          // The innermost match, not the outermost. `querySelectorAll` is in
          // document order, so an ancestor comes before its descendants and
          // taking the first one reads the launcher's whole container —
          // including the `/` shortcut badge beside the phrase.
          const all = [...document.querySelectorAll('header *, nav *')]
            .filter(e => /ask (the index a question|it something)/i.test(e.innerText || e.placeholder || ''));
          const el = all[all.length - 1];
          return el ? (el.innerText || el.placeholder || '').trim() : null;
        })()
        """
    )
    field = d.eval(
        """
        (() => {
          const el = [...document.querySelectorAll('input')]
            .find(i => /ask/i.test(i.placeholder || ''));
          return el ? el.placeholder.trim() : null;
        })()
        """
    )
    d.open_view("graph")
    rail = d.eval(
        """
        (() => {
          // Scoped to the graph's own rail. The top bar's launcher is a button
          // on every view, matches the same words, and comes first in document
          // order — so an unscoped search read the top bar twice and reported
          // it as disagreeing with itself.
          const el = [...document.querySelectorAll('.graph-rail a, .graph-rail button')]
            .find(e => /chunks it lives in|ask (the index|it)/i.test(e.innerText || ''));
          return el ? (el.innerText || '').trim() : null;
        })()
        """
    )
    # Read in two parts from 0.26.1, and the split is a judgement worth naming.
    #
    # The finding is that one destination had several names. The top bar's
    # launcher and the Search page's field are one control's invitation seen
    # twice, and those must still agree exactly — that is the bug, and it is
    # asserted first.
    #
    # The graph rail's button is a different journey: it does not open an empty
    # search, it runs one for the symbol you have selected. The v4 design names
    # it for what it gives you rather than echoing the search box, and a button
    # reading "Ask the index a question" told a reader nothing about what they
    # would get. So the rail is held to a different rule: it must not be a
    # third spelling of the search box's invitation. Require all three to match
    # and the only way to pass is to call the top bar "Chunks it lives in",
    # which is not an improvement anyone wants.
    if top_bar and field and top_bar != field:
        fail(
            "the top bar says %r and the search field says %r; they are one "
            "control's invitation and must read the same" % (top_bar, field)
        )
    if rail and rail in {top_bar, field}:
        fail(
            "the graph rail's button reads %r, the same words as the search "
            "box. It is a different journey — it searches for the selected "
            "symbol rather than opening an empty box — and naming it after the "
            "box says nothing about what it gives you." % rail
        )
    if rail and not re.search(r"chunks it lives in", rail, re.I):
        fail(
            "the graph rail's route into Search reads %r; the v4 design names "
            "it 'Chunks it lives in', for what the reader gets" % rail
        )


@finding("3.25", "the URL field's placeholder does not change after a failed attempt")
def _(d):
    d.open_view("index")
    d.open_index_panel("Add from a URL")
    selector = "(document.querySelector('#index-url') || {}).placeholder"
    first = d.eval(selector)
    if not first:
        fail("the 'Add from a URL' field carries no placeholder at all")

    d.type("#index-url", "not-a-url")
    d.click_text("button", "Fetch and index")
    d.wait_for(
        "!!document.querySelector('#index-url').closest('.card')"
        ".querySelector('.note.bad')",
        what="the failed attempt to be reported on the panel",
    )

    second = d.eval(selector)
    if second != first:
        fail(
            "the URL field's placeholder changed from %r to %r after a failed "
            "attempt" % (first, second)
        )
    d.close_index_panel("Add from a URL")


@finding("3.26", "naming a target store and 'each folder becomes its own store' are exclusive")
def _(d):
    d.open_view("index")
    # The dropdown by its own `aria-label`, not by its text: a `<select>`'s
    # `innerText` is the browser's business — Chrome gives an empty string for a
    # closed one — so matching on "add to" found nothing and the check reported
    # a control that was on the page the whole time.
    TARGET = 'select[aria-label="Where to index into"]'

    def target():
        return d.eval(
            "(() => { const s = document.querySelector(%s);"
            " if (!s) return null;"
            " return {value: ((s.selectedOptions[0] || {}).text || '').trim(),"
            "         disabled: s.disabled === true,"
            "         stores: [...s.options].filter(o => o.value !== 'each' && !o.disabled)"
            "                               .map(o => o.value)}; })()" % json.dumps(TARGET)
        )

    state = target()
    if state is None:
        fail("the Index page no longer carries a store dropdown beside its pickers")

    # The exclusivity the finding asks for, exercised rather than waited for.
    # The product's answer is in `reveal()`: opening the projects picker forces
    # the dropdown back to "each folder becomes its own store" and disables it,
    # because that mode makes each project its own store and an "add to <store>"
    # beside it is an instruction that contradicts the picker that is open.
    if not state["stores"]:
        skip("no open store is offered as an index target, so there is nothing to make exclusive")
    d.eval(
        "(() => { const s = document.querySelector(%s);"
        " s.value = %s;"
        " s.dispatchEvent(new Event('change', {bubbles: true})); })()"
        % (json.dumps(TARGET), json.dumps(state["stores"][0]))
    )
    named = target()
    if not named["value"].lower().startswith("add to"):
        fail(
            "the store dropdown would not take %r as its target; it reads %r"
            % (state["stores"][0], named["value"])
        )

    d.open_index_panel("Projects under a folder…")
    with_picker = target()
    if not with_picker["disabled"]:
        fail(
            "the store dropdown reads %r and is still live while 'Projects under "
            "a folder…' is open. That mode is documented to make each project its "
            "own store, and nothing reconciles the two." % with_picker["value"]
        )
    if with_picker["value"].lower().startswith("add to"):
        fail(
            "'Projects under a folder…' is open and the store dropdown still "
            "reads %r. It is disabled, so the contradiction is now unfixable "
            "from the page rather than resolved." % with_picker["value"]
        )

    d.close_index_panel("Projects under a folder…")
    if target()["disabled"]:
        fail(
            "closing 'Projects under a folder…' left the store dropdown disabled, "
            "so naming a target store is no longer possible at all"
        )


# ==========================================================================
# P4 — polish
# ==========================================================================


@finding("4.1", "a truncated path carries its full value in a title")
def _(d):
    for view in ("stores", "files"):
        d.open_view(view)
        d.wait_for("document.querySelectorAll('tbody tr').length > 0",
                   what="the %s table" % view, timeout=30)
        missing = d.eval(
            """
            [...document.querySelectorAll('tbody tr')].map(r => {
              const cell = [...r.querySelectorAll('td')]
                .find(td => /[\\\\/]/.test(td.innerText || ''));
              if (!cell) return null;
              const holder = cell.matches('[title]') ? cell : cell.querySelector('[title]');
              return holder && holder.getAttribute('title') ? null : (cell.innerText || '').trim();
            }).filter(Boolean)
            """
        )
        if missing:
            fail(
                "%d path cells on the %s page have no title tooltip, so the full "
                "value is only reachable through the DOM inspector. First: %r"
                % (len(missing), view, missing[0])
            )


@finding("4.2", "every Files column sorts")
def _(d):
    d.open_view("files")
    d.wait_for("document.querySelectorAll('tbody tr').length > 0", what="the files table")
    inert = d.eval(
        """
        [...document.querySelectorAll('table th')].map(th => {
          const sortable = th.hasAttribute('aria-sort') || !!th.querySelector('button')
                           || th.getAttribute('role') === 'columnheader' && th.tabIndex >= 0;
          return sortable ? null : (th.innerText || '').trim();
        }).filter(Boolean)
        """
    )
    if inert:
        fail(
            "these Files columns carry no sort affordance and clicking them does "
            "nothing: %s. Four of seven sorted; STORE, READ AS and LANGUAGE did "
            "not, and with no store filter on the page that left no way to look "
            "at one store's files at all." % ", ".join(inert)
        )


@finding("4.3", "the code preview shows that it scrolls")
def _(d):
    d.open_view("search")
    d.type(SEARCH_BOX, "release record sealed immutable")
    d.press("Enter")
    d.wait_for("document.querySelectorAll('pre, code, .preview').length > 0",
               what="a code preview")
    clipped = d.eval(
        """
        (() => {
          const panes = [...document.querySelectorAll('pre, .preview, .code')];
          const pane = panes.find(p => p.scrollWidth > p.clientWidth + 2);
          if (!pane) return null;
          const style = getComputedStyle(pane);
          const cue = style.overflowX === 'auto' || style.overflowX === 'scroll';
          const fade = !!pane.parentElement.querySelector('.fade, .shadow, .more');
          return {overflowX: style.overflowX, cue, fade};
        })()
        """
    )
    if clipped is None:
        skip("no preview pane in this result set is wider than its box")
    if not clipped["cue"] and not clipped["fade"]:
        fail(
            "the preview clips at the right edge with overflow-x: %s and no fade "
            "or shadow cue. It does scroll — the phone build shows a scrollbar — "
            "but on desktop nothing says the content continues." % clipped["overflowX"]
        )


@finding("4.4", "folder pickers sort case-insensitively")
def _(d):
    d.open_view("index")
    d.open_index_panel("Choose folders…")
    # Folders first and files after, each half by name — so the assertion is on
    # each half rather than on the whole list, which the route deliberately
    # groups. One name per row from the row's own `.name` span: a row can also
    # carry a store badge, and the whole row's text would sort that too.
    rows = d.eval(
        """
        [...document.querySelectorAll(%s)].map(entry => ({
          name: ((entry.querySelector('.name') || {}).textContent || '').trim(),
          // A file in a multiple-select picker is rendered inert; a folder is
          // the row with a tick beside it.
          folder: !entry.classList.contains('inert'),
        })).filter(row => row.name)
        """
        % json.dumps(OPEN_PICKER + " .entry")
    )
    folders = [row["name"] for row in rows if row["folder"]]
    files = [row["name"] for row in rows if not row["folder"]]
    if len(folders) < 2 and len(files) < 2:
        skip("the picker lists fewer than two entries of either kind here")
    for kind, names in (("folders", folders), ("files", files)):
        if names != sorted(names, key=lambda n: n.lower()):
            fail(
                "the folder picker sorts its %s case-sensitively, so every "
                "lowercase entry sinks below every uppercase one. Saw: %s"
                % (kind, ", ".join(names[:8]))
            )
    d.close_index_panel("Choose folders…")


@finding("4.5", "files in a folder picker read as not selectable")
def _(d):
    d.open_view("index")
    d.open_index_panel("Choose folders…")
    undimmed = d.eval(
        """
        (() => {
          const picker = document.querySelector(%s);
          if (!picker) return null;
          const entries = [...picker.querySelectorAll('.entry')];
          // The tick is a sibling of the row rather than a child of it, so
          // "has a checkbox" is a question about the wrapper: a folder row is
          // an `.entry-row`, and a file is a bare `.entry` beside it.
          const folders = entries.filter(e => e.closest('.entry-row'));
          const files = entries.filter(e => !e.closest('.entry-row'));
          if (!files.length || !folders.length) return null;
          const opacity = el => parseFloat(getComputedStyle(el).opacity || '1');
          const folderOpacity = Math.max(...folders.map(opacity));
          return files.filter(f => opacity(f) >= folderOpacity - 0.05)
                      .map(f => (f.innerText || '').trim()).slice(0, 3);
        })()
        """
        % json.dumps(OPEN_PICKER)
    )
    d.close_index_panel("Choose folders…")
    if undimmed is None:
        skip("this directory shows no mix of files and folders")
    if undimmed:
        fail(
            "files render at full contrast beside selectable folders, with a file "
            "icon and no checkbox: %s. Dimming them would say 'not selectable' "
            "without a second look." % ", ".join(undimmed)
        )


@finding("4.6", "the stat tiles do not wrap four-and-one at tablet width")
def _(d):
    d.set_viewport(768, 1024)
    try:
        d.open_view("stores", fresh=True)
        boxes = rects(d, ".tile, .stat, figure")
        if len(boxes) < 5:
            skip("fewer than five tiles are drawn on this page")
        rows = {}
        for box in boxes:
            rows.setdefault(round(box["top"]), []).append(box)
        shape = [len(v) for _, v in sorted(rows.items())]
        if shape[:2] == [4, 1]:
            fail(
                "at 768px the five Stores tiles break four across with ON DISK "
                "alone on a full-width row. 3+2 or 2+2+1 would read better. Saw %s"
                % shape
            )
    finally:
        d.reset_viewport()


@finding("4.7", "a store's root path is reachable on a phone")
def _(d):
    d.set_viewport(390, 844, mobile=True)
    try:
        d.open_view("stores", fresh=True)
        d.wait_for("document.querySelector('#root').innerText.length > 0",
                   what="the Stores view")
        body = view_text(d)
        roots = [
            r["path"]
            for row in stores(d)
            if not row.get("unopened")
            for r in row.get("roots", [])
        ]
        if not roots:
            skip("no open store has a registered root to show")
        shown = any(
            root in body or root.replace("\\", "/") in body.replace("\\", "/")
            for root in roots
        )
        exposed_elsewhere = d.eval(
            "[...document.querySelectorAll('[title], details, summary')]"
            ".some(el => /[\\\\/]/.test(el.getAttribute('title') || el.innerText || ''))"
        )
        if not shown and not exposed_elsewhere:
            fail(
                "the responsive table drops ROOTS and CHUNKS and nothing else "
                "exposes the root, so on a phone you cannot see what a store "
                "indexes. The product's answer is the `<span class=\"meta "
                "only-narrow\">` on the store's own row, which the stylesheet "
                "shows below 820px."
            )
        # Clipped *and* unrecoverable. Two kinds of overflow on this page are
        # not defects and were both being reported as one:
        #
        #   * `.sr-only` — the table's `<caption>` among others — is a 1×1 box
        #     with `overflow: hidden` by design, so its scrollWidth always
        #     exceeds its clientWidth. It is for a screen reader and cannot be
        #     clipped visually at all.
        #   * `lineCell(value, …)` deliberately truncates to one line and hangs
        #     the whole value on a `title`, which is finding 4.1's contract. A
        #     row that ends in an ellipsis and hands over its full text on hover
        #     is the design, not a clip.
        #
        # What is left is text cut off with nothing carrying the rest — which is
        # what would make a store's root unreadable on a phone.
        clipped = d.eval(
            """
            [...document.querySelectorAll('#root *')]
              .filter(el => el.children.length === 0)
              .filter(el => !el.closest('.sr-only'))
              .filter(el => el.scrollWidth > el.clientWidth + 2)
              .filter(el => {
                const full = (el.innerText || '').trim();
                const held = (el.closest('[title]') || {}).title || '';
                return !full || held.trim() !== full;
              })
              .map(el => (el.innerText || '').trim())
              .slice(0, 3)
            """
        )
        if clipped:
            fail(
                "text is clipped on the phone build with nothing carrying the rest "
                "of it — no `title`, no expansion: %s" % ", ".join(clipped)
            )
    finally:
        d.reset_viewport()


@finding("4.8", "store chips do not consume the phone's first screen")
def _(d):
    d.set_viewport(390, 844, mobile=True)
    try:
        d.open_view("search", fresh=True)
        # The store chips, and only those. This read `[aria-pressed], .chip`,
        # which is every chip-shaped control on the page and several that are
        # not above the results at all — the search dials, and from 0.27.0 the
        # sidebar's `Replay first-run screen`, which became a chip when the
        # muted text it used to be stopped reading as a control. Counting those
        # as store chips made the check report three rows where the store chips
        # occupied one, which is a failure about the wrong thing.
        chips = rects(d, ".store-chips .chip, .store-chips [aria-pressed]")
        store_chips = [c for c in chips if c["text"]]
        if len(store_chips) < 3:
            skip("fewer than three store chips are drawn here")
        tops = sorted({round(c["top"]) for c in store_chips})
        overflow = d.eval(
            "[...document.querySelectorAll('button, summary')]"
            ".some(b => /\\bmore\\b|\\+\\d+|show all/i.test(b.innerText || ''))"
        )
        if len(tops) > 2 and not overflow:
            fail(
                "%d store chips wrap to %d rows above the results on a 390px "
                "screen, with no overflow or 'more' affordance"
                % (len(store_chips), len(tops))
            )
    finally:
        d.reset_viewport()


@finding("4.9", "a phone result card is not covered by the summary bar")
def _(d):
    d.set_viewport(390, 844, mobile=True)
    try:
        d.open_view("search", fresh=True)
        d.type(SEARCH_BOX, "release record sealed immutable")
        d.press("Enter")
        d.wait_for("document.querySelectorAll(%s).length > 0" % json.dumps(RESULT_CARD),
                   what="search results")
        summary = d.eval(
            """
            (() => {
              const el = [...document.querySelectorAll('*')]
                .find(e => e.children.length === 0 && /\\bof\\b.*shown|budget/i.test(e.innerText || ''));
              if (!el) return null;
              const r = el.getBoundingClientRect();
              return {bottom: r.bottom, top: r.top,
                      fixed: getComputedStyle(el).position === 'fixed'};
            })()
            """
        )
        cards = rects(d, RESULT_CARD)
        if summary is None or not cards:
            skip("no summary bar or no result cards to compare")
        first = cards[0]
        if summary["fixed"] and first["top"] < summary["bottom"] - 1:
            fail(
                "the first result card starts at y=%.0f, under a fixed summary bar "
                "that ends at y=%.0f, so the card is half-covered"
                % (first["top"], summary["bottom"])
            )
    finally:
        d.reset_viewport()


@finding("4.10", "the queued-runs line matches what is queued, and clears")
def _(d):
    d.open_view("index")
    d.eval("new Promise(done => setTimeout(() => done(true), 800))")
    queued = len(d.api("/api/index/runs").get("queue") or [])
    body = view_text(d)
    stated = re.search(r"(\d+)\s+runs?\s+queued", body, re.IGNORECASE)

    if queued == 0 and stated:
        fail(
            "nothing is queued and the page still says %r. The line stayed after "
            "all runs finished, and read '1 run queued.' with four cards on screen."
            % stated.group(0)
        )
    if queued and not stated:
        fail("%d runs are queued and the page says nothing" % queued)
    if queued and int(stated.group(1)) != queued:
        fail("the page says %r while %d runs are actually queued"
             % (stated.group(0), queued))


@finding("4.11", "a run card's header and its log describe the same run")
def _(d):
    first = indexed_fixture(d, d.fixtures.small())
    second_run, same_store = start_index(d, d.fixtures.small())
    # Waited on by id: "the store's run" is now an ambiguous thing to wait for,
    # which is the whole of this finding.
    wait_for_run(d, same_store, run_id=second_run)
    if same_store != first:
        skip("the second submission made a new store, so there is no shared card")

    runs = d.api("/api/index/runs")["runs"]
    ids = [r["id"] for r in runs if r["store"] == same_store]
    if len(ids) < 2:
        fail(
            "after two runs against %s the daemon reports %d run(s) for it. Cards "
            "are keyed by store name rather than by run, which is why a card's "
            "header showed the new run's path and status while its body still "
            "held the previous run's log lines." % (same_store, len(ids))
        )

    # And the page draws one card per run rather than one per store, which is
    # what a card describing two different runs at once looked like.
    d.open_view("index")
    d.wait_for(
        "[...document.querySelectorAll(%s)].some(c => c.innerText.includes(%s))"
        % (json.dumps(RUN_CARD), json.dumps(same_store)),
        what="a run card for %s" % same_store,
    )
    drawn = d.eval(
        "[...document.querySelectorAll(%s)]"
        ".filter(c => c.innerText.includes(%s)).length"
        % (json.dumps(RUN_CARD), json.dumps(same_store))
    )
    if drawn < 2:
        fail(
            "the daemon holds %d runs for %s and the Index page draws %d card(s) "
            "for it. One card per store means the second run's header sits over "
            "the first run's log." % (len(ids), same_store, drawn)
        )


@finding("4.12", "a sub-second run does not report a rate against a one-second clock")
def _(d):
    store_name = indexed_fixture(d, d.fixtures.small())
    run = run_for(d, store_name)
    elapsed_ms = run.get("elapsed_ms") or 0
    if elapsed_ms >= 1000:
        skip("this run took %dms, so the timer's resolution is not in question" % elapsed_ms)

    d.open_view("index")
    card = d.eval(
        """
        (() => {
          const cards = [...document.querySelectorAll(%s)];
          const card = cards.find(c => c.innerText.includes(%s));
          return card ? card.innerText : null;
        })()
        """
        % (json.dumps(RUN_CARD), json.dumps(store_name))
    )
    if card is None:
        fail("no run card for %s was found" % store_name)
    if re.search(r"00:01", card) and re.search(r"chunks/s", card):
        fail(
            "a run that took %dms reports '00:01' and a chunks/s rate derived "
            "from it. A sub-second run needs either a finer unit or no rate at "
            "all." % elapsed_ms
        )


@finding("4.13", "/api/files?limit= refuses an out-of-range value instead of clamping")
def _(d):
    status, answer = d.api_result("/api/files?limit=2000")
    if status == 200:
        returned = len(answer.get("files", []))
        fail(
            "asking for limit=2000 answered 200 with %d rows and no indication it "
            "was clamped. The neighbouring `offset` refuses an out-of-range value "
            "with a 400 and a reasoned comment in the source; `limit` should say "
            "so too." % returned
        )
    want("an out-of-range limit", status, 400)
    if "limit" not in json.dumps(answer).lower():
        fail("the refusal does not mention `limit`: %s" % json.dumps(answer)[:200])


@finding("4.14", "every table has a caption or an accessible name")
def _(d):
    for view in ("stores", "files", "doctor", "about"):
        d.open_view(view)
        d.eval("new Promise(done => setTimeout(() => done(true), 600))")
        nameless = d.eval(
            """
            [...document.querySelectorAll('table')].map((t, i) => {
              const named = t.querySelector('caption')
                || t.getAttribute('aria-label')
                || t.getAttribute('aria-labelledby');
              return named ? null : i;
            }).filter(v => v !== null)
            """
        )
        if nameless:
            fail(
                "the %s page has %d table(s) with no caption and no aria-label. "
                "Accessibility was otherwise clean across all ten views, which is "
                "what makes this the one thing left." % (view, len(nameless))
            )


@finding("4.15", "'Chunks it lives in' is a button, because it behaves like one")
def _(d):
    d.open_view("graph")
    tag = d.eval(
        """
        (() => {
          // The wording moved when finding 3.24 gave the destination one
          // name; what this check is about is the element type, so it finds
          // the control by either phrasing.
          const el = [...document.querySelectorAll('.graph-rail a, .graph-rail button')]
            .find(e => /chunks it lives in|ask the index/i.test(e.innerText || ''));
          return el ? el.tagName : null;
        })()
        """
    )
    if tag is None:
        skip("the graph rail does not offer that control for the default selection")
    if tag != "BUTTON":
        fail(
            "'Chunks it lives in' is an <%s> styled as a button with "
            "preventDefault. It works, but the element type does not match the "
            "behaviour." % tag.lower()
        )


@finding("4.16", "there is a way to act on the whole result set, not just the page")
def _(d):
    d.open_view("files")
    d.wait_for("document.querySelectorAll('tbody tr').length > 0", what="the files table")
    total = d.api("/api/files?limit=1")["total"]
    if total <= 15:
        skip("the corpus fits on one page, so the distinction does not arise")

    d.eval(
        "(document.querySelector('thead input[type=checkbox]') || {click(){}}).click()"
    )
    d.eval("new Promise(done => setTimeout(() => done(true), 600))")
    body = view_text(d)
    # The page groups thousands, so 1664 is drawn as "1,664". The separator is
    # the page's, and the number is the assertion.
    grouped = "{:,}".format(total).replace(",", "[,\u202f ]?")
    if not re.search(r"select all %s|all %s matches" % (grouped, grouped), body, re.IGNORECASE):
        fail(
            "the header checkbox selects the page (the bulk bar honestly says so) "
            "and nothing offers to act on all %d matches" % total
        )


@finding("4.17", "a failed adopt keeps the picker open where it was")
def _(d):
    folder = d.fixtures.monorepo()  # a folder that is deliberately not a store
    d.open_view("stores")
    d.click_text("button", "Adopt existing .semlith")
    d.wait_for("!!document.querySelector(%s)" % json.dumps(OPEN_PICKER + " .crumbs"),
               what="the adopt picker")

    # Walked somewhere first, because "keeps the picker open where it was" can
    # only be told from "reopens at $HOME" when it is not standing at $HOME.
    descend_picker(d, folder)
    before = picker_where(d)
    want("the adopt picker's folder before the attempt", before, folder)

    # The portal's own path, through the picker, so the error handling under
    # test is the page's and not this drive's. The button says what it does
    # here, rather than the multi-select picker's "Use this folder".
    d.click_text(OPEN_PICKER + " button", "Adopt this directory")
    d.wait_for(
        "!!document.querySelector('.note.bad')",
        what="the failed adopt to be reported",
    )
    # The page hides the card, reports the failure and reopens where it was, in
    # that order, so the reopen is given its own moment before it is judged.
    try:
        d.wait_for("!!document.querySelector(%s)" % json.dumps(OPEN_PICKER), timeout=10)
    except cdp.ProtocolError:
        pass  # the assertion below says what that means

    if not exists(d, OPEN_PICKER):
        fail(
            "a failed adopt closed the picker and dropped back to the Stores "
            "page. Reopening starts again at $HOME with all navigation lost."
        )
    after = picker_where(d)
    if after != before:
        fail("the picker stayed open but moved from %r to %r" % (before, after))
    d.click_text(OPEN_PICKER + " button", "Close")


@finding("4.18", "the adopt picker says which folder is adoptable")
def _(d):
    folder = d.fixtures.adoptme()
    listing = d.api("/api/dirs?path=%s" % folder)
    entries = listing.get("entries") or listing.get("dirs") or []
    names = [e.get("name") for e in entries]
    marked = [e for e in entries if e.get("store") or e.get("adoptable") or e.get("semlith")]

    if ".semlith" not in names and not marked:
        fail(
            "inside a folder that holds a valid `.semlith`, the picker lists %s "
            "and nothing distinguishes it from any other folder. This is the "
            "surface of finding 2.1: the picker hides the thing it adopts."
            % (", ".join(n for n in names if n) or "nothing")
        )


# ------------------------------------------------------------------ 0.26.0
#
# The v4 surfaces. These are not findings from the 2026-09-17 drive — they are
# the pages 0.26.0 added, checked the same way and numbered after it, so the
# gate covers what shipped rather than only what was once broken. Each one
# leaves a screenshot behind, which is what the release record carries.


@finding("5.1", "the sidebar is the thirteen pages, in four groups, in order")
def _(d):
    """Updated for 0.26.1, which took the v3 design's four groups back.

    The thirteen entries are unchanged and this still asserts every one of
    them: what moved is the grouping. v4 flattened v3's Workspace / Explore /
    Operate / Account into two groups of six and seven, which is a list with
    two headings in it rather than a menu. `Account` held License and About;
    the binary is free and has no licence page, so the fourth group is the two
    pages that describe this machine.
    """
    d.open_view("stores")
    labels = [t for t in texts_of(d, ".sidebar .nav-item") if t]
    expected = [
        "Stores",
        "Files",
        "Inside the index",
        "Search",
        "Graph",
        "Impact",
        "Retrieval ledger",
        "Reports",
        "Agents",
        "Cloud",
        "Privacy",
        "Doctor",
        "About",
    ]
    want("the sidebar's entries", labels, expected)
    # The group labels are uppercased by the stylesheet, so innerText reads
    # them that way. The design's names are what is being asserted, not the
    # typography.
    groups = [t.title() for t in texts_of(d, ".sidebar .nav-group-label") if t]
    want("the sidebar's groups", groups, ["Workspace", "Explore", "Operate", "Machine"])


@finding("5.2", "Impact answers for a symbol, by hop, with a support class on every row")
def _(d):
    path = d.fixtures.small()
    indexed_fixture(d, path)

    # The busiest symbol in the store, which is how the Graph page picks the
    # neighbourhood it lands on. A hand-picked name would be a check of the
    # fixture rather than of the page.
    overview = d.api("/api/graph?limit=1")
    nodes = overview.get("nodes") or []
    if not nodes:
        skip("the store's graph holds no symbol to read backwards from")
    symbol = nodes[0].get("name")

    d.open_view("impact")
    body = view_text(d)
    if "Reverse reachability" not in body:
        fail("the Impact page did not render its lead copy: %s" % body[:200])

    d.eval(
        "(() => { const box = document.querySelector('.impact-band input[type=search]');"
        " box.value = %s;"
        " box.dispatchEvent(new Event('input', {bubbles: true}));"
        " [...document.querySelectorAll('.impact-band button')]"
        "   .find(b => b.textContent.trim() === 'Reach').click(); })()"
        % json.dumps(symbol)
    )
    deadline = time.time() + 30
    while time.time() < deadline:
        if "Reading" not in view_text(d):
            break
        time.sleep(0.5)

    body = view_text(d)
    if "reach" not in body.lower():
        fail("Impact said nothing about %s: %s" % (symbol, body[:300]))
    subject = text_of(d, ".impact-subject", "the symbol Impact is about")
    want("the subject line", subject, symbol)

    rows = d.eval(
        "document.querySelectorAll('.impact-block:not(.impact-files) .impact-row').length"
    )
    if not rows:
        # Nothing reaching the busiest symbol is a legitimate answer, and the
        # page has to say so rather than render an empty list.
        if "nothing in this store reaches" not in body.lower():
            fail("no rows and no sentence saying why: %s" % body[:300])
        return

    # Every reached row carries one of the four support classes. A hop with
    # no badge is a hop a reader will assume was verified.
    unbadged = d.eval(
        "[...document.querySelectorAll('.impact-block:not(.impact-files) .impact-row')]"
        ".filter(el => !el.querySelector('.conf')).length"
    )
    if unbadged:
        fail("%d of %d reached rows carry no support class" % (unbadged, rows))
    # And the hop groups are in order, nearest first.
    hops = [t for t in texts_of(d, ".impact-block:not(.impact-files) .impact-hop h2") if t and t[0].isdigit()]
    numbers = [int(t.split()[0]) for t in hops]
    if numbers != sorted(numbers):
        fail("the hop groups are out of order: %s" % numbers)


@finding("5.3", "the path finder and Trace render on the Impact page")
def _(d):
    d.open_view("impact")
    body = view_text(d)
    for wanted in ["Path finder", "Prefer verified edges", "Strict", "Trace", "Copy as evidence"]:
        if wanted not in body:
            fail("the Impact page is missing %r: %s" % (wanted, body[:300]))


@finding("5.4", "the Graph page lists the store's communities")
def _(d):
    d.open_view("graph")
    time.sleep(3)
    body = view_text(d)
    if "Map" not in body:
        fail("the Graph page has no Map panel: %s" % body[:300])
    shown = text_of(d, ".map-shown", "the Map panel's count")
    if "Shown" not in shown and "No call or import edges" not in view_text(d):
        fail("the Map panel does not say how many of how many it shows: %r" % shown)


@finding("5.5", "Inside the index states what the graph covers")
def _(d):
    d.open_view("index")
    # The three cards are a scan of every call edge away, so this waits for
    # them rather than sleeping a fixed time and calling a slow store a bug.
    # The three cards are a scan of every call edge away, so this waits for
    # them rather than sleeping a fixed time and calling a slow store a bug.
    # Compared case-blind: the card titles are uppercased by the stylesheet,
    # and innerText reads what is rendered.
    deadline = time.time() + 30
    while time.time() < deadline:
        if "language mix" in view_text(d).lower():
            break
        time.sleep(0.5)
    body = view_text(d).lower()
    for wanted in ["language mix", "chunks by month indexed", "graph health"]:
        if wanted not in body:
            fail("Inside the index is missing the %r card" % wanted)
    if "call targets with no definition here" not in body:
        fail("Graph health does not state its unresolved targets: %s" % body[:400])
    # Every bar is sized through the CSSOM, because the portal is served
    # under `style-src 'self'` and a width written into the markup is
    # blocked — silently, leaving every bar full width.
    #
    # Asserted as what is on screen rather than as what is in the markup: a
    # width assigned through the CSSOM and one written into the attribute
    # look identical in `outerHTML`, and only one of them is applied. A bar
    # whose share is under 100% and whose rendered width equals its track's
    # is a bar the policy dropped.
    narrower = d.eval(
        "[...document.querySelectorAll('.mix-row .meter')].some(track => {"
        "  const fill = track.firstElementChild; if (!fill) return false;"
        "  const w = fill.getBoundingClientRect().width;"
        "  const t = track.getBoundingClientRect().width;"
        "  return t > 0 && w > 0 && w < t - 1; })"
    )
    if not narrower:
        fail(
            "every language-mix bar fills its whole track, so the width was "
            "written into the markup and `style-src 'self'` dropped it"
        )


@finding("5.6", "the ledger lists sessions, filters them, and exports what it shows")
def _(d):
    d.open_view("ledger")
    time.sleep(1)
    body = view_text(d)
    if "Sessions" not in body:
        fail("the ledger page has no per-session table")
    for wanted in ["Markdown", "CSV", "JSON"]:
        if wanted not in body:
            fail("the sessions table cannot export %s" % wanted)
    selects = d.eval("document.querySelectorAll('.card.pad .filters select').length")
    if selects < 3:
        fail("the sessions table has %d filter controls, expected client, tier and model" % selects)


@finding("5.7", "Session replay is off until Privacy turns it on")
def _(d):
    d.open_view("ledger")
    time.sleep(1)
    # From 0.27.0 the rows and the replay are two tabs over one ledger rather
    # than two stacked cards, so the replay's copy is behind its tab. The check
    # presses it, which is what a reader does.
    body = view_text(d)
    if "Session replay" not in body:
        fail("the ledger page has no Session replay tab")
    opened = d.eval(
        """
        (() => {
          const tab = [...document.querySelectorAll('.tab')]
            .find(t => /session replay/i.test(t.textContent || ''));
          if (!tab) return false;
          tab.click();
          return true;
        })()
        """
    )
    if not opened:
        fail("the ledger has no Session replay tab to open")
    time.sleep(0.5)
    body = view_text(d)
    if "Turn on under Privacy" not in body:
        fail("Session replay does not say where it is turned on: %s" % body[:300])
    state = d.api("/api/ledger/replay")
    if state.get("enabled"):
        skip("session replay is already on on this machine, so its off state cannot be checked")
    if state.get("sessions"):
        fail("session replay is off and returned sessions anyway: %s" % state)


@finding("5.8", "Reports generates all five, locally")
def _(d):
    d.open_view("reports")
    body = view_text(d)
    for wanted in [
        "Retrieval savings",
        "AI access audit",
        "Change brief",
        "Index health",
        "Knowledge gaps",
    ]:
        if wanted not in body:
            fail("the Reports page is missing %r" % wanted)
    if "Nothing leaves the machine" not in body:
        fail("the Reports page does not say where the data came from")
    for kind in ["savings", "access", "change", "health", "gaps"]:
        answer = d.api("/api/report?kind=%s&format=markdown" % kind)
        text = answer.get("text") or ""
        if not text.strip():
            fail("the %s report generated nothing" % kind)
        if "Generated" not in text:
            fail("the %s report does not say when it was generated" % kind)


@finding("5.9", "the Cloud page describes the service and contacts nothing")
def _(d):
    d.open_view("cloud")
    body = view_text(d)
    if "Semlith Cloud is one hosted store" not in body:
        fail("the Cloud page did not render: %s" % body[:300])
    if "not connected" not in body:
        fail("the Cloud page does not say it is not connected")
    for word in ["Disconnect", "token prefix", "acme/api"]:
        if word in body:
            fail("the Cloud page drew its connected state, which this release has no client for")


@finding("5.10", "the Agents page measures what its tool list costs")
def _(d):
    d.open_view("agents")
    body = view_text(d)
    if "What the tool list costs" not in body:
        fail("the Agents page does not state what the tool list costs")
    if "paid" in body:
        fail("a tool on the Agents page is marked paid")
    listed = d.api("/api/agents")
    tools = listed.get("tools") or []
    if len(tools) != 16:
        fail("the Agents page lists %d tools, expected 16" % len(tools))


@finding("5.11", "every new page renders in both themes")
def _(d):
    """The record's evidence, taken by the gate rather than by hand.

    One screenshot per new surface in light and again in dark. The check
    fails only if a page does not render at all — the pictures are what a
    person reads, and they are what the release record carries.
    """
    pages = [
        ("impact", "Impact"),
        ("graph", "Graph"),
        ("index", "Inside the index"),
        ("ledger", "Retrieval ledger"),
        ("reports", "Reports"),
        ("cloud", "Cloud"),
        ("agents", "Agents"),
        ("privacy", "Privacy"),
    ]
    for theme in ("light", "dark"):
        d.eval("document.documentElement.setAttribute('data-theme', %s)" % json.dumps(theme))
        for view, title in pages:
            d.open_view(view)
            time.sleep(1.5)
            body = view_text(d)
            if title not in body:
                fail("%s did not render in the %s theme" % (title, theme))
            d.shot("5.11-%s-%s" % (theme, view))
    d.eval("document.documentElement.removeAttribute('data-theme')")


# ---------------------------------------------------------------- 0.26.1
#
# The 6.x block is the 2026-09-21 design-parity drive: the portal opened page
# by page beside `Semlith Portal v4.dc.html` and driven at seven widths. Where
# the 5.x checks assert that a page exists, these assert that it says what the
# design says and that nothing on it is out of reach.


def open_welcome(d):
    """The first-run screen, which is not a view and has no sidebar.

    `open_view` waits for a heading inside the shell; this screen replaces the
    shell entirely, so it is navigated to and awaited by its own heading.
    """
    d.navigate("%s/?token=%s#welcome" % (d.portal_url, d.token))
    d.wait_for(
        "(() => { const h = document.querySelector('#root h1');"
        " return !!h && (h.textContent || '').trim() === 'No stores yet'; })()",
        what="the first-run screen's heading",
    )
    d.eval("new Promise(done => requestAnimationFrame(() => done(true)))")


@finding("6.1", "the first-run screen carries everything the v4 lockup and card carry")
def _(d):
    """0.26.0 shipped this screen with the version, one step, the ledger
    sentence and the route into adopting a store all missing, and with a
    footer that named the address without the port it was serving on."""
    open_welcome(d)
    body = view_text(d)
    about = d.api("/api/about")

    version = text_of(d, ".welcome .lockup .ver", "the version beside the mark")
    want("the version beside the mark", version, "v" + about["version"])

    steps = d.eval("document.querySelectorAll('.welcome .step').length")
    if steps != 4:
        fail("the first-run screen draws %d steps, and the v4 design draws 4" % steps)

    if "--no-ledger" not in body:
        fail(
            "the first-run screen does not say the ledger records locally. It is on by "
            "default, so the screen that introduces the product is where that is said."
        )
    if "Adopt an existing .semlith" not in body:
        fail(
            "the first-run screen offers no way to adopt a store that already exists, "
            "so the one screen whose job is to open a first store offers only one way"
        )

    host = d.eval("location.host")
    foot = text_of(d, ".welcome .foot", "the first-run footer")
    if host not in foot:
        fail(
            "the footer reads %r and does not name %s. 'loopback only' is a claim the "
            "reader cannot check without the port." % (foot, host)
        )


@finding("6.2", "Skip for now lands on Stores")
def _(d):
    """It went to About, which is the page about the binary rather than the
    page the reader was skipping ahead to."""
    open_welcome(d)
    d.click_text(".welcome button", "Skip for now")
    d.wait_for(
        "(location.hash || '') === '#stores'",
        what="Skip for now to land on Stores",
    )


@finding("6.3", "the Stores table offers the way into what the index holds")
def _(d):
    d.open_view("stores")
    label = "See what is actually inside the index"
    if label not in view_text(d):
        fail("the Stores table has no route into Inside the index")
    d.click_text(".table-follow", label)
    d.wait_for("(location.hash || '') === '#index'", what="the route into Inside the index")


@finding("6.4", "the Retrieval ledger offers the way into Reports")
def _(d):
    d.open_view("ledger")
    if "Build a report" not in view_text(d):
        fail("the Retrieval ledger header has no route into Reports")
    d.click_text(".page-head button", "Build a report")
    d.wait_for("(location.hash || '') === '#reports'", what="the route into Reports")


@finding("6.5", "About states the licence the binary ships under")
def _(d):
    """0.27.0 took the MCP revisions row off this page and this check with it.

    The row named a wire contract an agent settles in its handshake and a person
    never acts on, and the v4 design's About page is seven facts and a language
    table. `/api/about` still returns `revisions`, so nothing that reads them
    lost anything — which is why this check no longer looks for them on the
    page. See 7.9, which asserts they are gone.
    """
    d.open_view("about")
    body = view_text(d)
    about = d.api("/api/about")
    if about["license"] not in body:
        fail("the About page does not state the licence the binary ships under")


@finding("6.6", "the sidebar states whether the ledger is recording")
def _(d):
    """0.27.0 took the `Replay first-run screen` control out of the sidebar and
    this check's second half with it.

    It was added in 0.26.1 on the reasoning that without it the first-run screen
    is unreachable once a store exists. That is still true, and it was judged
    not to be worth a permanent control in the sidebar of every page — the
    screen is a first run, and `#welcome` still reaches it. What the daemon card
    says about recording is the part of this finding that was about the sidebar
    doing its job, and it is kept.
    """
    d.open_view("stores")
    card = text_of(d, "#daemon-stores", "the daemon card's second line")
    recording = (d.api("/api/about")).get("ledger") is not False
    want("the daemon card's ledger state", "ledger on" in card, recording)


@finding("6.7", "Reports previews the one report that is selected")
def _(d):
    """The page used to be five cards each with its own Generate button and one
    preview under them all, so the preview could be showing any of the five."""
    d.open_view("reports")
    types = d.eval("document.querySelectorAll('.report-type').length")
    if types != 5:
        fail("the Reports picker offers %d report types, expected 5" % types)
    d.wait_for(
        "((document.querySelector('.report-text') || {}).textContent || '').length > 40",
        what="the selected report to generate",
    )
    first = d.eval("document.querySelector('.report-text').textContent")
    # The card's own text is its name, its blurb and its reader run together,
    # so the name is what is matched; the click bubbles to the card.
    d.click_text(".report-type .name", "Index health")
    d.wait_for(
        "((document.querySelector('.report-text') || {}).textContent || '')"
        " !== %s" % json.dumps(first),
        what="the preview to follow the selected report",
    )
    name = text_of(d, ".report-preview-card .report-bar .name", "the preview's file name")
    if not name.endswith(".md"):
        fail("the preview names %r, which is not the chosen Markdown format" % name)
    d.click_text(".report-builder .chip", "CSV")
    d.wait_for(
        "(document.querySelector('.report-preview-card .report-bar .name').textContent || '')"
        ".endsWith('.csv')",
        what="the format chip to change the file written",
    )


@finding("6.8", "no page scrolls sideways, at any width the design supports")
def _(d):
    """A control pushed off the right edge is a control nobody can reach, and
    the page scrollbar that comes with it makes every page feel broken. Seven
    widths, because 0.26.0 was verified at one."""
    widths = [390, 430, 820, 1024, 1280, 1440, 1920]
    views = list(cdp.Drive.VIEW_TITLES)
    bad = []
    try:
        for width in widths:
            d.set_viewport(width, 844 if width < 600 else 900, mobile=width < 600)
            for view in views:
                d.open_view(view, fresh=True)
                time.sleep(0.4)
                seen = d.eval(
                    "({page: document.documentElement.scrollWidth,"
                    " vw: document.documentElement.clientWidth})"
                )
                # One pixel of slack: a fractional layout width rounds up and
                # is not a horizontal scrollbar.
                if seen["page"] > seen["vw"] + 1:
                    bad.append("%s at %dpx scrolls to %dpx" % (view, width, seen["page"]))
    finally:
        d.reset_viewport()
    if bad:
        fail("pages scroll sideways: %s" % "; ".join(bad))


@finding("6.9", "no page writes an error to the browser console")
def _(d):
    """A console error is a defect a screenshot cannot show. 0.26.0 was never
    read for them, so this reads every page for them once.

    The console is read only once a page has stopped fetching. Navigating
    away from a page with a request still in flight cancels it, and Chrome
    logs the cancellation against whichever page it lands on — on Windows
    that showed up as `ERR_CONNECTION_RESET` on `/api/privacy`, whose socket
    scan is the slowest read in the portal. Waiting is the honest fix: a load
    failure that survives a quiet network is a real one, and is still failed
    on. Suppressing the message by name would have hidden the real thing too.
    """

    def settle(limit=15.0):
        """Block until the page stops making requests, or `limit` passes."""
        deadline = time.time() + limit
        last, stable = -1, 0
        while time.time() < deadline:
            count = d.eval("performance.getEntriesByType('resource').length")
            stable = stable + 1 if count == last else 0
            last = count
            # Three readings the same, a beat apart: enough for a page whose
            # panels fetch one after another rather than all at once.
            if stable >= 3:
                return True
        # Said rather than silently tolerated: a page still fetching after
        # fifteen seconds is worth knowing about even if nothing errored.
        return False

    bad, restless = [], []
    for view in cdp.Drive.VIEW_TITLES:
        d.open_view(view, fresh=True)
        if not settle():
            restless.append(view)
        # Cleared after the page is quiet, so anything read below was written
        # by this page rather than by the navigation that reached it.
        d.clear_console()
        time.sleep(0.5)
        for line in d.console_errors():
            bad.append("%s: %s" % (view, line))
    if bad:
        fail("the console carried errors: %s" % "; ".join(bad[:8]))
    if restless:
        fail(
            "these pages were still fetching after 15s, so their console was "
            "never read against a settled page: %s" % ", ".join(restless)
        )


@finding("6.10", "an index run outlives the page that started it")
def _(d):
    """A run lives in the daemon, not in the tab.

    The Index page says so in as many words — "leaving, refreshing or closing
    the tab changes nothing, and a run ends only on its Stop". It was true
    when 0.24.0 moved the run out of the streaming response that used to *be*
    it, and nothing since should have moved it back. This is the check that
    says so, because the failure mode is invisible until someone reloads
    mid-run and watches their work disappear.
    """
    path = d.fixtures.bulk()
    started = d.api("/api/index", method="POST", body={"path": [path]})
    runs = started.get("runs") or []
    if not runs:
        skip("the daemon queued no run for the bulk fixture")
    store = runs[0].get("store")

    d.open_view("index", fresh=True)
    before = d.eval(
        "document.querySelectorAll('.run-card').length"
    )
    if not before:
        fail("the Index page drew no run card for a run the daemon had just accepted")

    # A full document load, which is what a reload and a reopened tab both are.
    d.navigate("%s/#index" % d.portal_url)
    d.wait_for(
        "!!document.querySelector('.run-card')",
        what="the run card to come back after a reload; a run that vanishes with "
        "the page is a run bound to the request that started it, which is the "
        "defect 0.24.0 fixed",
    )
    after = d.eval(
        "[...document.querySelectorAll('.run-card .card-title')].map(n => n.textContent)"
    )
    if store not in after:
        fail(
            "after reloading, the Index page lists %r and not the running store %r"
            % (after, store)
        )

    # And the daemon still owns it, which is the half a screenshot cannot show.
    live = d.api("/api/index/runs")
    if not any(r.get("store") == store for r in (live.get("runs") or [])):
        fail("the daemon dropped the run for %s when the page reloaded" % store)


# ---------------------------------------------------------------- 0.27.0
#
# The 7.x block. Where 6.x asserted that each page says what the design says,
# these assert the things 0.27.0 changed underneath every page at once — the
# shared flex rule, the control shapes, the type scale, the bottom floor — plus
# the two surfaces it built from nothing.
#
# The point of writing them here rather than checking them by hand once: every
# one of these was found by opening the portal and looking at it, and the next
# portal release should inherit the gate instead of finding them again.


@finding("7.1", "a page whose content overflows scrolls to its end")
def _(d):
    """`.view > *` set `flex-shrink: 0` and `.scroller` put it back at equal
    specificity and later in the file, so on any page that overflowed one child
    absorbed the whole overflow: the page stopped scrolling and the scroller was
    crushed toward zero with `.card { overflow: hidden }` clipping what was
    inside it.

    The Retrieval ledger was where it showed, but it was never a ledger bug —
    five pages put a `.scroller` directly under `.view`. So this is asserted on
    the ledger *and* on a second page, which is the whole reason it is one
    check with a loop rather than two checks.
    """
    for view in ("ledger", "stores"):
        d.open_view(view)
        d.eval("(document.querySelector('.view') || {}).scrollTop = 1e6")
        d.eval("new Promise(done => requestAnimationFrame(() => done(true)))")
        crushed = d.eval(
            """
            (() => {
              const s = document.querySelector('.view > .scroller');
              if (!s) return null;
              const r = s.getBoundingClientRect();
              return r.height < 40 ? r.height : 0;
            })()
            """
        )
        if crushed:
            fail(
                "on %s the scroller under .view is %dpx tall — it absorbed the "
                "page's overflow instead of the page scrolling" % (view, crushed)
            )
        # Every card that is on the page is a card that can be read: nothing
        # below the fold may be clipped to nothing by the same rule.
        clipped = d.eval(
            """
            [...document.querySelectorAll('.view .card')].filter(el => {
              const r = el.getBoundingClientRect();
              return r.height > 0 && el.scrollHeight > Math.ceil(r.height) + 2;
            }).length
            """
        )
        if clipped:
            fail("on %s, %d card(s) are shorter than their own content" % (view, clipped))


@finding("7.2", "every page ends with the design's floor under its last card")
def _(d):
    """`.view` was `20px 20px 24px` against the design's `24px 24px 44px`, so
    every page ended 20px short and the last card sat against the viewport edge.

    The graph page is the one recorded exception: it is `.graph-page`, has no
    padding at all by design, and its canvas fills the frame.
    """
    views = ("stores", "ledger", "reports", "impact", "about", "privacy")
    # Two widths, because the narrow breakpoint sets `.view`'s padding again
    # and used to keep the old 24px while the base rule moved. A card against
    # the bottom edge is truer on a phone than anywhere else.
    try:
        for width in (1280, 390):
            d.set_viewport(width, 844 if width < 600 else 900, mobile=width < 600)
            for view in views:
                d.open_view(view, fresh=True)
                floor = d.eval(
                    "(() => { const v = document.querySelector('.view');"
                    " return v ? getComputedStyle(v).paddingBottom : null; })()"
                )
                if floor is None:
                    fail("%s has no .view to measure a floor on" % view)
                want("the floor under %s at %dpx" % (view, width), floor, "44px")
    finally:
        d.reset_viewport()

    # The graph page is the one recorded exception and must stay one: it is
    # `.graph-page`, its canvas fills the frame, and a 44px band under it would
    # be a band of empty panel.
    d.open_view("graph")
    if exists(d, ".graph-page > .view"):
        fail("the graph page now roots on .view, so the recorded exception no longer applies")


@finding("7.3", "no control in the portal renders as bare text")
def _(d):
    """`.button.ghost` had no background and no border, which is what made the
    graph page's `Reset` read as a caption rather than as a control. It is gone
    and all fourteen call sites take the secondary shape.

    A pressed chip is the design's ink fill rather than a blue wash, and does
    not change font weight — a chip that gained weight on press changed width on
    press, so picking one in a wrapped row reflowed the row under the pointer.
    """
    for view in ("stores", "graph", "ledger", "reports", "impact", "agents"):
        d.open_view(view)
        bare = d.eval(
            """
            [...document.querySelectorAll('.view .button, .graph-page .button,'
              + ' .search-page .button')].filter(el => {
              const s = getComputedStyle(el);
              const noFill = s.backgroundColor === 'rgba(0, 0, 0, 0)'
                          || s.backgroundColor === 'transparent';
              const noEdge = s.borderTopWidth === '0px'
                          || s.borderTopStyle === 'none'
                          || s.borderTopColor === 'rgba(0, 0, 0, 0)';
              return noFill && noEdge;
            }).map(el => (el.textContent || '').trim()).slice(0, 5)
            """
        )
        if bare:
            fail("on %s these controls have neither a background nor a border: %r" % (view, bare))
        weights = d.eval(
            """
            (() => {
              const on = [...document.querySelectorAll('.chip[aria-pressed="true"]')];
              const off = [...document.querySelectorAll('.chip[aria-pressed="false"]')];
              if (!on.length || !off.length) return null;
              return [getComputedStyle(on[0]).fontWeight,
                      getComputedStyle(off[0]).fontWeight];
            })()
            """
        )
        if weights and weights[0] != weights[1]:
            fail(
                "on %s a pressed chip is weight %s and an unpressed one is %s, so a "
                "chip row reflows when one is picked" % (view, weights[0], weights[1])
            )


@finding("7.4", "no code block fades out at its right edge")
def _(d):
    """`pre.code` carried a 28px right-edge mask to say "the line continues".
    Every block that class draws is a command a user is meant to select and
    copy, and the fade made its last characters unreadable whether or not they
    were the end of the line. The design's `<pre>` blocks scroll plainly.

    The phone-only `.store-chips` mask is not a code block and is left alone.
    """
    for view in ("agents", "privacy"):
        d.open_view(view)
        masked = d.eval(
            """
            [...document.querySelectorAll('pre.code, .report-text, .copyfield .text,'
              + ' .hit pre, .brief-text, .code-line, .log')].filter(el => {
              const s = getComputedStyle(el);
              return (s.maskImage && s.maskImage !== 'none')
                  || (s.webkitMaskImage && s.webkitMaskImage !== 'none');
            }).length
            """
        )
        if masked:
            fail("on %s, %d code surface(s) still fade at the right edge" % (view, masked))


@finding("7.5", "the Impact page draws its canvas beside the answer")
def _(d):
    """The right column of this page was a large empty area: the design puts a
    320px reverse-reachability canvas at the top of it and the implementation
    never drew one at all, while the `Changing` row and the three figures sat in
    two unboxed page-wide bands above both columns.
    """
    d.open_view("impact")
    if not exists(d, ".impact-canvas-card"):
        fail("the Impact page draws no canvas card")
    caption = text_of(d, ".impact-canvas-card .canvas-caption", "the canvas caption")
    want("the canvas caption", caption, "reverse reachability, rings by hop")
    # The design's 320px, as a floor rather than as an exact height. It is drawn
    # against a five-node mock; a real store answers with dozens, and the rings
    # need the room to keep their labels apart — so the card grows with the
    # viewport and stops at 460.
    height = d.eval(
        "Math.round(document.querySelector('.impact-canvas-card').getBoundingClientRect().height)"
    )
    if height < 320 or height > 460:
        fail("the canvas card is %dpx tall; it should sit between 320 and 460" % height)
    # The `Changing` line and the three figures belong to a card in the left
    # column, not to a band across the page.
    if not exists(d, ".impact-subject-card .impact-subject-row"):
        fail("the `Changing` row is not inside the left column's card")
    if not exists(d, ".impact-subject-card .impact-stats"):
        fail("the three figures are not inside the left column's card")
    d.shot("7.5-impact-canvas")


@finding("7.6", "the canvas rings the reached set by hop")
def _(d):
    """The caption is the specification. The design's own painter is the shared
    force simulation with no hop input at all — one ring of everything, then
    physics — so the picture it drew was not the picture it promised.

    This asserts the thing the caption claims: ask a real store a question that
    reaches at least two hops, and the canvas must put the further hop further
    out.
    """
    # A symbol this corpus really has callers for, asked of the store rather
    # than hard-coded. A name that reaches nothing is a fact about whatever
    # happens to be indexed, and a check that failed on it would be measuring
    # the fixture.
    graph = d.api("/api/graph")
    reachable = None
    for node in (graph.get("nodes") or [])[:40]:
        answer = d.api("/api/impact?name=%s&depth=3" % node["name"])
        if ((answer.get("impact") or {}).get("reached") or []):
            reachable = node["name"]
            break
    if not reachable:
        skip(
            "no symbol in the open store is reached by anything, so there is no "
            "question to put to the canvas"
        )

    d.open_view("impact")
    driven = d.eval(
        """
        (() => {
          const box = document.querySelector('.impact-band input[type="search"]');
          if (!box) return false;
          box.value = %s;
          box.dispatchEvent(new Event('input', {bubbles: true}));
          box.dispatchEvent(new KeyboardEvent('keydown', {key: 'Enter', bubbles: true}));
          return true;
        })()
        """
        % json.dumps(reachable)
    )
    if not driven:
        fail("the Impact page has no symbol field to drive")
    d.wait_for(
        "!!document.querySelector('.impact-results .impact-row')",
        what="a reached row on the Impact page for %s" % reachable,
    )
    painted = d.eval(
        """
        (() => {
          const c = document.querySelector('.impact-canvas');
          if (!c) return 0;
          const ctx = c.getContext('2d');
          const d = ctx.getImageData(0, 0, c.width, c.height).data;
          let ink = 0;
          for (let i = 3; i < d.length; i += 4) if (d[i] > 0) ink += 1;
          return ink;
        })()
        """
    )
    if painted < 500:
        fail(
            "the Impact canvas is blank after asking about %s, which returned rows"
            % reachable
        )
    # The three figures are the same answer counted, so a canvas that drew and
    # a card that did not would be two readings of one question.
    reached = text_of(d, ".impact-subject-card .impact-stat .n", "the Reached figure")
    if reached.strip() in ("", "0"):
        fail("the Changing card still reads 0 Reached while the table has rows")
    d.shot("7.6-impact-rings")


@finding("7.7", "the Reports builder offers a window and a scope, and the route takes them")
def _(d):
    """The page offered neither, and `/api/report` read a `store` parameter and
    threw it away — a control the route ignored would have been worse than no
    control, which is why the route work landed with the page.
    """
    d.open_view("reports")
    for group, what in (("window", "a Window group"), ("scope", "a Scope group")):
        if not exists(d, "[data-group='%s'] .chip" % group):
            fail("the Reports builder offers no %s" % what)
    formats = d.eval("document.querySelectorAll(\"[data-group='format'] .chip\").length")
    want("the formats the builder offers", formats, 5)
    # The route has to answer differently, not just accept the argument.
    week = d.api("/api/report?kind=change&window=week")
    month = d.api("/api/report?kind=change&window=month")
    if week.get("text") == month.get("text"):
        fail("/api/report returns the same change brief for a week and for a month")


@finding("7.8", "a schedule the page adds is a schedule the daemon runs")
def _(d):
    """Schedules are the first persistent, daemon-owned, time-driven state in
    the product, and the one thing that makes them worth having is that the
    daemon really runs them. The card reflects what the daemon holds rather than
    page state, so this reads the list back out of the route rather than off the
    page it was typed into.
    """
    d.open_view("reports")
    if not exists(d, ".schedules-card"):
        fail("the Reports page has no Schedules card")
    held = d.api("/api/schedules")
    listed = d.eval("document.querySelectorAll('.schedules-card .schedule-row').length")
    want("the rows the card draws", listed, len(held.get("schedules") or {}))
    # A schedule whose destination has gone away says so on the card. Silent
    # failure here is the worst available outcome: a schedule that reads `on`
    # beside a folder that never fills.
    broken = [s for s in (held.get("schedules") or {}).values() if s.get("last_error")]
    if broken:
        body = view_text(d)
        for schedule in broken:
            if "failed" not in body.lower():
                fail("a schedule recorded an error and the card does not say so")
    d.shot("7.8-schedules")


@finding("7.9", "About has lost the two blocks the v4 page has no place for")
def _(d):
    """A forty-eight-row catalogue of embedding models, of which this machine
    has fetched one, and a row of MCP protocol revisions an agent settles in its
    handshake. Both routes still answer — what went is a table, not a capability.
    """
    d.open_view("about")
    if exists(d, ".w-models"):
        fail("the About page still draws the models table")
    if "MCP revisions" in view_text(d):
        fail("the About page still states the MCP revisions")
    # The capability is untouched, which is the whole argument for removing the
    # view: assert the routes rather than trusting the sentence.
    if not (d.api("/api/models").get("models") or []):
        fail("/api/models stopped answering when its portal view was removed")
    if not (d.api("/api/about").get("revisions") or []):
        fail("/api/about stopped returning the MCP revisions")


@finding("7.10", "an unreadable store does not take the readable ones down with it")
def _(d):
    """One store whose database cannot be read made every route that aggregates
    over `with_fleet` return `500 {"error":"disk I/O error"}`, so the Graph page
    drew nothing and the message named neither the store nor the fact that the
    others were fine.

    The fixture for this is a damaged store, which the drive does not build —
    when there is none registered, there is nothing to assert and the check says
    so rather than passing quietly.
    """
    answer = d.api("/api/stores")
    unreadable = [s for s in (answer.get("stores") or []) if s.get("unreadable")]
    if not unreadable:
        skip("no unreadable store is registered, so there is nothing to answer around")
    for route in ("/api/graph", "/api/files"):
        got = d.api(route)
        failures = got.get("failed") or []
        if not failures:
            fail("%s answered with no failures listed while a store is unreadable" % route)
        for entry in failures:
            if not entry.get("store") or not entry.get("path"):
                fail("%s reports a failure that names neither the store nor its path" % route)
