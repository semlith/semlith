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
import re
import time

CHECKS = []


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
    for run in d.api("/api/index/runs")["runs"]:
        if run["store"] == store_name:
            return run
    return None


TERMINAL = {"done", "stopped", "failed"}


def wait_for_run(d, store_name, timeout=600):
    """Block until a store's run is over, and return its final snapshot."""
    deadline = time.time() + timeout
    last = None
    while time.time() < deadline:
        last = run_for(d, store_name)
        if last and str(last.get("status", "")).lower() in TERMINAL:
            return last
        time.sleep(0.5)
    fail(
        "the run on %s never finished within %ds (last status: %s)"
        % (store_name, timeout, last and last.get("status"))
    )


def indexed_fixture(d, path):
    """Index a fixture folder and wait for it, returning the store name."""
    _, store_name = start_index(d, path)
    wait_for_run(d, store_name)
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
        "!!document.querySelector('#root').innerText.match(/%s/)" % re.escape(store_name),
        what="the finished run card for %s" % store_name,
    )

    buttons = d.eval(
        """
        (() => {
          const cards = [...document.querySelectorAll('[data-run], .run, article')];
          const card = cards.find(c => /done|stopped|failed/i.test(c.innerText)
                                    && c.innerText.includes(%s));
          if (!card) return null;
          return [...card.querySelectorAll('button')].map(b => (b.innerText || '').trim());
        })()
        """
        % json.dumps(store_name)
    )
    if buttons is None:
        fail("no run card for %s in a terminal state was found on the Index page" % store_name)

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
    # can be correlated — by requiring both to be machine-readable with an
    # explicit offset. A fix that labels both as local satisfies it too.
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

    stamped = re.compile(r"\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})$")
    for row in rows[:20]:
        when = str(row.get("at") or row.get("time") or row.get("timestamp") or "")
        if not stamped.match(when):
            fail(
                "a ledger row is timestamped %r, which carries no zone. The CLI "
                "printed UTC and the portal printed local time with no marker on "
                "either, so a portal event could not be lined up with a ledger "
                "row." % when
            )

    d.open_view("stores")
    body = view_text(d)
    times = re.findall(r"\b\d{2}:\d{2}:\d{2}\b", body)
    if times and not re.search(r"\b(UTC|GMT|local|[A-Z]{2,5}T)\b", body):
        fail(
            "the Stores page prints wall-clock times (%s) with no zone anywhere "
            "on the page" % ", ".join(times[:3])
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
    adopted = [row for row in stores(d) if normalise(row["dir"]).startswith(normalise(folder))
               or normalise(folder) in [normalise(r["path"]) for r in row.get("roots", [])]]
    if not adopted:
        fail("the adopt answered 200 but no store on /api/stores came from %s" % folder)


@finding("2.2", "the URL panel names its own store, and a private address is refused as one")
def _(d):
    d.open_view("index")
    has_own_store_control = d.eval(
        """
        (() => {
          const panels = [...document.querySelectorAll('section, fieldset, .panel, .group')];
          const panel = panels.find(p => /add from a url/i.test(p.innerText || ''));
          if (!panel) return null;
          return !!panel.querySelector('select, [role=combobox], input[list]');
        })()
        """
    )
    if has_own_store_control is None:
        fail("no 'Add from a URL' panel was found on the Index page")
    if not has_own_store_control:
        fail(
            "the 'Add from a URL' panel still has no store selector of its own. "
            "The control it reads sits in a different control group above, and "
            "the error it produces names the stores without saying where to name one."
        )

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
    d.click_text("button, a", "Projects under a folder…")
    d.wait_for("!!document.querySelector('dialog, .picker, [role=dialog]')",
               what="the projects picker")

    has_up = d.eval(
        "[...document.querySelectorAll('dialog button, [role=dialog] button')]"
        ".some(b => /^(up|\\.\\.|back|parent)$/i.test((b.innerText||'').trim()))"
    )
    clickable = d.eval(
        """
        (() => {
          const picker = document.querySelector('dialog, [role=dialog], .picker');
          if (!picker) return null;
          const entries = [...picker.querySelectorAll('.entry, li, tr')];
          return entries.some(e => e.querySelector('button, a') ||
                                   e.getAttribute('role') === 'button' ||
                                   e.tabIndex >= 0);
        })()
        """
    )
    if not has_up:
        fail(
            "the projects picker has no Up control, so it cannot leave the home "
            "directory. Its sibling, Choose folders…, has full navigation."
        )
    if not clickable:
        fail(
            "folder names in the projects picker are not clickable, so there is "
            "no way to descend into a monorepo. They rendered as SPAN.name "
            "inside SPAN.entry with only a checkbox."
        )
    # And, having navigated, it finds the two repositories and the plain folder.
    listing = d.api("/api/projects?path=%s" % monorepo)
    found = {normalise(p).rsplit("/", 1)[-1] for p in listing.get("projects", listing.get("paths", []))}
    for expected in ("repo-one", "repo-two"):
        if expected not in found:
            fail("the projects listing for the monorepo fixture is missing %s: saw %s"
                 % (expected, sorted(found)))


@finding("2.6", "finished runs can be dismissed and the active run sorts first")
def _(d):
    finished = indexed_fixture(d, d.fixtures.small())
    d.open_view("index")

    before = d.eval("document.querySelectorAll('[data-run], .run, article').length")
    d.click_text("button", "Remove")
    d.eval("new Promise(done => setTimeout(() => done(true), 600))")
    after = d.eval("document.querySelectorAll('[data-run], .run, article').length")
    if after >= before:
        fail(
            "Remove on a finished run card left %d cards where there were %d. "
            "With no dismiss and no auto-collapse, four runs are four full cards "
            "and the live one is off-screen." % (after, before)
        )

    # And when something is actually running, it is the card at the top.
    _, busy = start_index(d, d.fixtures.bulk())
    d.open_view("index")
    d.eval("new Promise(done => setTimeout(() => done(true), 800))")
    first = d.eval(
        "(document.querySelector('[data-run], .run, article') || {}).innerText || ''"
    )
    if busy not in first:
        fail(
            "a run that is still going (%s) is not the first card on the page; "
            "the top card reads %r" % (busy, first[:120])
        )
    d.api("/api/index/control", method="POST", body={"store": busy, "action": "stop"})


@finding("2.7", "lang:, path: and budget re-run the query like every other filter")
def _(d):
    d.open_view("search")
    d.type("input[type=search], #query, input[name=query]", "release record sealed immutable")
    d.press("Enter")
    d.wait_for("document.querySelectorAll('[data-hit], .hit, article').length > 0",
               what="search results")
    before = texts_of(d, "[data-hit], .hit, article")

    path_field = d.eval(
        """
        (() => {
          const inputs = [...document.querySelectorAll('input')];
          const el = inputs.find(i => /path/i.test(i.name || i.id || i.placeholder || '')
                                   && i.type !== 'search');
          return el ? (el.id ? '#' + el.id : ('input[name="' + el.name + '"]')) : null;
        })()
        """
    )
    if not path_field:
        fail("no `path:` filter field was found on the Search page")

    d.type(path_field, "**/*.md")
    # Deliberately no Enter in the main query box: that is exactly the
    # workaround the finding says a user should not need.
    d.eval("new Promise(done => setTimeout(() => done(true), 1500))")
    after = texts_of(d, "[data-hit], .hit, article")

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


@finding("2.8", "the graph's scope field applies as you type")
def _(d):
    # The ambiguity: a visible Apply affordance would also answer the finding's
    # complaint, which is that a field silently waiting for Enter reads as
    # broken next to click-to-apply chips. This asserts the reading the
    # finding's own sentence gives — that typing should do something — so a
    # release that ships an Apply button instead must update this check.
    d.open_view("graph")
    d.wait_for("/\\d+\\s+symbols/i.test(document.querySelector('#root').innerText)",
               what="the graph summary")
    before = view_text(d)

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
        fail("no 'Scope to a path, or find a symbol' field was found on the Graph page")

    d.type(scope, "release")
    d.eval("new Promise(done => setTimeout(() => done(true), 2000))")
    if view_text(d) == before:
        fail(
            "typing into the graph's scope field changed nothing after two "
            "seconds. Every other filter on the page is a click-to-apply chip, "
            "so a text field that only answers Enter reads as broken."
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
    panel = d.eval(
        """
        (() => {
          const all = [...document.querySelectorAll('section, fieldset, .panel, .card')];
          const p = all.find(el => /logical core/i.test(el.innerText || ''));
          return p ? p.innerText : null;
        })()
        """
    )
    if panel is None:
        fail("no machine limits panel was found on the Index page")

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


@finding("3.3", "'threads each' recomputes when 'runs at once' changes")
def _(d):
    d.open_view("index")
    runs_field = d.eval(
        """
        (() => {
          const el = [...document.querySelectorAll('input, select')]
            .find(i => /runs.?at.?once/i.test((i.name || '') + (i.id || '')));
          return el ? (el.id ? '#' + el.id : ('[name="' + el.name + '"]')) : null;
        })()
        """
    )
    if not runs_field:
        fail("no 'runs at once' field was found on the Index page")

    d.type(runs_field, "3")
    d.press("Enter")
    d.eval("new Promise(done => setTimeout(() => done(true), 1200))")

    help_text = d.eval(
        """
        (() => {
          const all = [...document.querySelectorAll('p, small, .help, .hint')];
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


@finding("3.4", "a value the daemon calls saved, the portal calls saved too")
def _(d):
    d.open_view("index")
    panel = d.eval(
        """
        (() => {
          const all = [...document.querySelectorAll('section, fieldset, .panel, .card')];
          const p = all.find(el => /runs at once/i.test(el.innerText || ''));
          return p ? p.innerText : null;
        })()
        """
    )
    if panel is None:
        fail("no indexing settings panel was found on the Index page")
    # 3.3 has just written a value, so this run of the drive has a saved one.
    if re.search(r"\bderived\b", panel) and not re.search(r"\bsaved\b", panel):
        fail(
            "the panel calls the runs-at-once value 'derived' when settings.json "
            "holds a saved one. The daemon logs it as '(saved)', and the two only "
            "diverge when the saved value equals the derived one — exactly when a "
            "user is trying to work out whether their setting took effect:\n%s"
            % panel[:300]
        )


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
    selector = (
        "(() => { const el = [...document.querySelectorAll('input')]"
        ".find(i => /url|link to a page/i.test(i.placeholder || '')); "
        "return el ? el.placeholder.trim() : null; })()"
    )
    first = d.eval(selector)
    if first is None:
        fail("no 'Add from a URL' field was found on the Index page")

    d.type("input[placeholder=%s]" % json.dumps(first), "not-a-url")
    d.click_text("button", "Fetch and index")
    d.eval("new Promise(done => setTimeout(() => done(true), 1500))")

    second = d.eval(selector)
    if second != first:
        fail(
            "the URL field's placeholder changed from %r to %r after a failed "
            "attempt" % (first, second)
        )


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
    d.type("input[type=search], #query, input[name=query]", "release record sealed immutable")
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
    d.click_text("button, a", "Choose folders…")
    d.wait_for("!!document.querySelector('dialog, [role=dialog], .picker')",
               what="the folder picker")
    names = d.eval(
        """
        (() => {
          const picker = document.querySelector('dialog, [role=dialog], .picker');
          return [...picker.querySelectorAll('.name, .entry, li')]
            .map(e => (e.innerText || '').trim().split('\\n')[0]).filter(Boolean);
        })()
        """
    )
    if len(names) < 2:
        skip("the picker lists fewer than two entries here")
    if names != sorted(names, key=lambda n: n.lower()):
        fail(
            "the folder picker sorts case-sensitively, so every lowercase entry "
            "sinks below every uppercase one. Saw: %s" % ", ".join(names[:8])
        )
    d.press("Escape")


@finding("4.5", "files in a folder picker read as not selectable")
def _(d):
    d.open_view("index")
    d.click_text("button, a", "Choose folders…")
    d.wait_for("!!document.querySelector('dialog, [role=dialog], .picker')",
               what="the folder picker")
    undimmed = d.eval(
        """
        (() => {
          const picker = document.querySelector('dialog, [role=dialog], .picker');
          const entries = [...picker.querySelectorAll('.entry, li')];
          const files = entries.filter(e => !e.querySelector('input[type=checkbox]'));
          const folders = entries.filter(e => e.querySelector('input[type=checkbox]'));
          if (!files.length || !folders.length) return null;
          const opacity = el => parseFloat(getComputedStyle(el).opacity || '1');
          const folderOpacity = Math.max(...folders.map(opacity));
          return files.filter(f => opacity(f) >= folderOpacity - 0.05)
                      .map(f => (f.innerText || '').trim()).slice(0, 3);
        })()
        """
    )
    d.press("Escape")
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
        d.type("input[type=search], #query, input[name=query]", "release record sealed immutable")
        d.press("Enter")
        d.wait_for("document.querySelectorAll('[data-hit], .hit, article').length > 0",
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
        cards = rects(d, "[data-hit], .hit, article")
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
    wait_for_run(d, same_store)
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
          const cards = [...document.querySelectorAll('[data-run], .run, article')];
          const card = cards.find(c => c.innerText.includes(%s));
          return card ? card.innerText : null;
        })()
        """
        % json.dumps(store_name)
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
    d.click_text("button, a", "Adopt existing .semlith")
    d.wait_for("!!document.querySelector('dialog, [role=dialog], .picker')",
               what="the adopt picker")
    before = d.eval(
        "(document.querySelector('dialog, [role=dialog], .picker')"
        " .querySelector('.path, header, h2') || {}).innerText || ''"
    )
    d.api_result("/api/adopt", method="POST", body={"path": folder})
    # The portal's own path, through the picker, so the error handling under
    # test is the page's and not this drive's.
    d.click_text("dialog button, [role=dialog] button", "Use")
    d.eval("new Promise(done => setTimeout(() => done(true), 1200))")

    still_open = d.eval("!!document.querySelector('dialog, [role=dialog], .picker')")
    if not still_open:
        fail(
            "a failed adopt closed the picker and dropped back to the Stores "
            "page. Reopening starts again at $HOME with all navigation lost."
        )
    after = d.eval(
        "(document.querySelector('dialog, [role=dialog], .picker')"
        " .querySelector('.path, header, h2') || {}).innerText || ''"
    )
    if after != before:
        fail("the picker stayed open but moved from %r to %r" % (before, after))
    d.press("Escape")


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
