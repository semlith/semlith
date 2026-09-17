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
    relative = os.path.relpath(path, here)
    if relative.startswith(".."):
        fail(
            "the picker is showing %s, which is not above %s, so there is no way "
            "to walk down to it" % (here, path)
        )
    for part in relative.split(os.sep):
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
    """One spelling of a path, so a Windows comparison is about the path."""
    return path.replace("\\", "/").rstrip("/").lower()


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
    open_stores = [row for row in stores(d) if not row.get("unopened")]
    if len(open_stores) < 2:
        skip("the ledger multiplier only shows with two or more stores open")

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
    store_name = indexed_fixture(d, d.fixtures.small())
    run = run_for(d, store_name)
    held = store_named(d, store_name)

    if run.get("indexed") != held["files"]:
        fail(
            "the run card says it handled %s files and %s holds %d. A card that "
            "under-reports makes the user's own index run look like it did "
            "almost nothing." % (run.get("indexed"), store_name, held["files"])
        )
    if run.get("chunks") != held["chunks"]:
        fail(
            "the run card says %s chunks and %s holds %d"
            % (run.get("chunks"), store_name, held["chunks"])
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
    ledger = d.api("/api/ledger")
    rows = ledger.get("rows")
    if rows is None:
        fail(
            "/api/ledger returns no rows, so the portal cannot show the ledger "
            "at all (see finding 3.16) and its timestamps cannot be compared "
            "with the CLI's"
        )
    if not rows:
        skip("nothing has been recorded in the ledger yet")

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
    open_stores = [row["name"] for row in stores(d) if not row.get("unopened") and row["files"] > 0]
    if len(open_stores) < 2:
        skip("telling a store filter from no filter needs two stores with files")
    target = open_stores[0]

    d.open_view("stores")
    opened = d.eval(
        """
        (() => {
          const rows = [...document.querySelectorAll('tbody tr')];
          const row = rows.find(r => r.innerText.includes(%s));
          if (!row) return 'no row';
          const kebab = row.querySelector('button[aria-haspopup], button[aria-label*="more" i], td:last-child button');
          if (!kebab) return 'no kebab';
          kebab.click();
          return 'opened';
        })()
        """
        % json.dumps(target)
    )
    if opened != "opened":
        fail("could not open the kebab menu on the %s row: %s" % (target, opened))
    d.click_text("button, a, [role=menuitem]", "Open in Files")
    d.wait_for("location.hash.startsWith('#files')", what="the Files view")
    d.eval("new Promise(done => setTimeout(() => done(true), 800))")

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


@finding("2.6", "finished runs can be dismissed and the active run sorts first")
def _(d):
    finished = indexed_fixture(d, d.fixtures.small())
    d.open_view("index")
    d.wait_for(
        "[...document.querySelectorAll(%s)].some(c => c.innerText.includes(%s))"
        % (json.dumps(RUN_CARD), json.dumps(finished)),
        what="the finished run card for %s" % finished,
    )

    before = d.eval("document.querySelectorAll(%s).length" % json.dumps(RUN_CARD))
    # The card's own Remove, not the "Remove all finished" button above the
    # cards and not a queued run's Remove: scoped to a card, and a hidden
    # button reads as empty text so only the one on offer can match.
    d.click_text(RUN_CARD + " button", "Remove")
    d.wait_for(
        "document.querySelectorAll(%s).length < %d" % (json.dumps(RUN_CARD), before),
        what="the dismissed card to go",
    )
    after = d.eval("document.querySelectorAll(%s).length" % json.dumps(RUN_CARD))
    if after >= before:
        fail(
            "Remove on a finished run card left %d cards where there were %d. "
            "With no dismiss and no auto-collapse, four runs are four full cards "
            "and the live one is off-screen." % (after, before)
        )

    # And when something is actually running, it is the card at the top.
    _, busy = start_index(d, d.fixtures.bulk())
    d.open_view("index")
    d.wait_for(
        "[...document.querySelectorAll(%s)].some(c => c.innerText.includes(%s))"
        % (json.dumps(RUN_CARD), json.dumps(busy)),
        what="the run card for %s" % busy,
    )
    first = d.eval("(document.querySelector(%s) || {}).innerText || ''" % json.dumps(RUN_CARD))
    if busy not in first:
        fail(
            "a run that is still going (%s) is not the first card on the page; "
            "the top card reads %r" % (busy, first[:120])
        )
    d.api("/api/index/control", method="POST", body={"store": busy, "action": "stop"})


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
          return {button: button ? (button.innerText || '').trim() : null,
                  describedby: described,
                  hint: hint ? (hint.innerText || '').trim() : null};
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
    row_text = d.eval(
        "(() => { const r = [...document.querySelectorAll('tbody tr')]"
        ".find(r => r.innerText.includes(%s)); return r ? r.innerText : null; })()"
        % json.dumps(store_name)
    )
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
    d.wait_for("/callers/i.test(document.querySelector('#root').innerText)",
               what="the graph's CALLERS rail", timeout=30)
    kinds = d.eval(
        """
        (() => {
          const heads = [...document.querySelectorAll('h1,h2,h3,h4,th,.rail-title')];
          const head = heads.find(h => /^callers$/i.test((h.innerText||'').trim()));
          if (!head) return null;
          const list = head.parentElement;
          return [...list.querySelectorAll('li, tr, .row')]
            .map(r => (r.innerText || '').trim()).filter(Boolean);
        })()
        """
    )
    if not kinds:
        skip("nothing is listed under CALLERS for the default selection")
    # The ambiguity: renaming the heading to "incoming edges" answers the
    # finding too. This takes the other half of the finding's own sentence —
    # "or should filter to `calls`" — because a list called CALLERS that holds
    # non-calls is the part that misleads.
    wrong = [row for row in kinds if re.search(r"\b(defines|references|imports|contains|aliases)\b", row)]
    if wrong:
        fail(
            "the CALLERS list holds %d edges that are not calls, for example %r. "
            "The section is really 'incoming edges'; either it says so or it "
            "filters to calls." % (len(wrong), wrong[0][:120])
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
    d.open_view("graph")
    d.wait_for("/\\d+\\s+symbols/i.test(document.querySelector('#root').innerText)",
               what="the graph summary")

    def counts():
        body = view_text(d)
        symbols = re.search(r"([\d,]+)\s+symbols", body, re.IGNORECASE)
        edges = re.search(r"([\d,]+)\s+edges", body, re.IGNORECASE)
        if not symbols or not edges:
            fail("the graph summary no longer prints symbol and edge counts")
        return (int(symbols.group(1).replace(",", "")), int(edges.group(1).replace(",", "")))

    base_symbols, base_edges = counts()
    scope = d.eval(
        """
        (() => {
          const el = [...document.querySelectorAll('input')]
            .find(i => /scope|symbol/i.test(i.placeholder || i.name || i.id || ''));
          return el ? (el.id ? '#' + el.id : ('input[placeholder="' + el.placeholder + '"]')) : null;
        })()
        """
    )
    if not scope:
        fail("no graph scope field was found")
    d.type(scope, "acquire")
    d.press("Enter")
    d.eval("new Promise(done => setTimeout(() => done(true), 1500))")
    scoped_symbols, scoped_edges = counts()

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
    rows = d.api("/api/doctor").get("clients") or d.api("/api/doctor").get("rows") or []
    if not rows:
        skip("the doctor route lists no clients on this machine")
    by_state = {}
    for row in rows:
        state = str(row.get("state") or row.get("status") or "").lower().strip()
        fix = (row.get("fix") or row.get("command") or "").strip()
        if not fix or fix == "—":
            continue
        by_state.setdefault(state, {}).setdefault(fix, []).append(row.get("name"))
    for state, fixes in by_state.items():
        if len(fixes) > 1:
            listed = "; ".join(
                "%s → %r" % (", ".join(names), fix) for fix, names in fixes.items()
            )
            fail(
                "the state %r is given %d different remedies with nothing to "
                "explain the difference: %s" % (state, len(fixes), listed)
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


@finding("3.15", "the models table carries a size for the model actually in use")
def _(d):
    # The ambiguity: the finding complains that 43 of 48 rows are empty and
    # that sorting by SIZE hides the five that are not. It does not say every
    # model must gain a size — upstream may not publish one. The two things it
    # does imply are asserted: the model semlith is running has a size, and
    # the sort puts the rows that have one where they can be seen.
    about = d.api("/api/about")
    in_use = about.get("model") or (about.get("embedding") or {}).get("model")
    if not in_use:
        skip("/api/about does not name the model in use")

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

    d.open_view("about")
    d.click_text("th, th button", "SIZE")
    d.eval("new Promise(done => setTimeout(() => done(true), 600))")
    column = d.eval(
        """
        [...document.querySelectorAll('table')]
          .filter(t => /size/i.test(t.innerText))
          .slice(-1)
          .flatMap(t => [...t.querySelectorAll('tbody tr')])
          .map(r => (r.querySelector('td:nth-child(3)') || {}).innerText || '')
          .map(s => s.trim())
        """
    )
    known = [i for i, value in enumerate(column) if value and value != "—"]
    unknown = [i for i, value in enumerate(column) if not value or value == "—"]
    if known and unknown and min(unknown) < max(known):
        fail(
            "sorting the models table by SIZE puts rows with no size before rows "
            "that have one, so the sort control hides the only data in the column"
        )


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
        skip("the privacy scan finds nothing to refuse in this corpus")

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

    toggle = "button[aria-label*='theme' i], [data-theme-toggle], button[title*='theme' i]"
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


@finding("3.24", "one destination has one name")
def _(d):
    d.open_view("search")
    top_bar = d.eval(
        """
        (() => {
          const el = [...document.querySelectorAll('header *, nav *')]
            .find(e => /ask (the index a question|it something)/i.test(e.innerText || e.placeholder || ''));
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
          const el = [...document.querySelectorAll('a, button')]
            .find(e => /chunks it lives in|ask (the index|it)/i.test(e.innerText || ''));
          return el ? (el.innerText || '').trim() : null;
        })()
        """
    )
    phrases = {p for p in (top_bar, field, rail) if p}
    if len(phrases) > 1:
        fail(
            "one destination is given %d names: %s. Whichever wording wins, the "
            "top bar, the search field and the graph rail button should agree."
            % (len(phrases), ", ".join(sorted(repr(p) for p in phrases)))
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
    state = d.eval(
        """
        (() => {
          const select = [...document.querySelectorAll('select')]
            .find(s => /add to|each folder becomes its own store/i.test(s.innerText || ''));
          const projects = [...document.querySelectorAll('button, a')]
            .find(b => /projects under a folder/i.test(b.innerText || ''));
          if (!select || !projects) return null;
          const value = (select.selectedOptions[0] || {}).text || '';
          return {value: value.trim(), projectsDisabled: projects.disabled === true,
                  selectDisabled: select.disabled === true};
        })()
        """
    )
    if state is None:
        fail("the Index page no longer carries both a store dropdown and a projects picker")
    names_a_store = state["value"].lower().startswith("add to")
    if names_a_store and not state["projectsDisabled"]:
        fail(
            "the store dropdown reads %r while 'Projects under a folder…' is still "
            "offered. That mode is documented to make each project its own store, "
            "and nothing reconciles the two." % state["value"]
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
                "indexes"
            )
        clipped = d.eval(
            "[...document.querySelectorAll('#root *')]"
            ".filter(el => el.children.length === 0 && el.scrollWidth > el.clientWidth + 2)"
            ".map(el => (el.innerText || '').trim()).slice(0, 3)"
        )
        if clipped:
            fail("text is clipped mid-word on the phone build: %s" % ", ".join(clipped))
    finally:
        d.reset_viewport()


@finding("4.8", "store chips do not consume the phone's first screen")
def _(d):
    d.set_viewport(390, 844, mobile=True)
    try:
        d.open_view("search", fresh=True)
        chips = rects(d, "[aria-pressed], .chip")
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
          const el = [...document.querySelectorAll('a, button')]
            .find(e => /chunks it lives in/i.test(e.innerText || ''));
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
    if not re.search(r"select all %d|all %d matches" % (total, total), body, re.IGNORECASE):
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
