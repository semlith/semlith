"""A small Chrome DevTools Protocol client, and the object the checks drive.

This exists because the portal drive has to click real buttons in a real
browser, and every off-the-shelf way of doing that drags in a package tree
larger than semlith itself. The protocol is a JSON request/response over one
WebSocket, so the whole client is about three hundred lines.

Only what the drive needs is implemented. If a check needs something that is
not here, add the one method it needs rather than reaching for a framework.

The one third-party dependency is `websocket-client`, which is imported as
`websocket`. Everything else is the standard library.

    pip install websocket-client
"""

import base64
import json
import os
import platform
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

try:
    import websocket  # type: ignore
except ImportError:  # pragma: no cover - the message is the whole point
    # Deliberately not fatal at import: `drive.py --list` should still work on
    # a machine that has not installed anything yet. It becomes fatal the
    # moment something actually tries to open a socket.
    websocket = None


class BrowserMissing(Exception):
    """No Chrome or Chromium could be found or started."""


class ProtocolError(Exception):
    """The browser answered a command with an error."""


# --------------------------------------------------------------- the browser


def find_browser():
    """The Chrome or Chromium binary to drive.

    `CHROME_PATH` wins, so a machine with an unusual install, or a CI image
    that ships a pinned build, does not have to be guessed about.
    """
    override = os.environ.get("CHROME_PATH")
    if override:
        if not os.path.exists(override):
            raise BrowserMissing(
                "CHROME_PATH is set to %s, and there is nothing there." % override
            )
        return override

    system = platform.system()
    if system == "Darwin":
        candidates = [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
            "/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary",
        ]
        for path in candidates:
            if os.path.exists(path):
                return path
    elif system == "Windows":
        program_files = [
            os.environ.get("PROGRAMFILES", r"C:\Program Files"),
            os.environ.get("PROGRAMFILES(X86)", r"C:\Program Files (x86)"),
        ]
        for base in program_files:
            path = os.path.join(base, "Google", "Chrome", "Application", "chrome.exe")
            if os.path.exists(path):
                return path
    else:
        for name in ("google-chrome", "google-chrome-stable", "chromium", "chromium-browser"):
            found = shutil.which(name)
            if found:
                return found

    raise BrowserMissing(
        "no Chrome or Chromium was found on this machine. Install one, or point "
        "CHROME_PATH at the binary you want the drive to use."
    )


def free_port():
    """A port nobody is listening on right now.

    There is a race between closing this socket and Chrome binding the port,
    and it is the ordinary one every test harness lives with.
    """
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


# ------------------------------------------------------------------- the drive


class Drive:
    """A headless browser, an HTTP client for the portal, and a screenshot pad.

    Every check gets one of these as its only argument. It carries the portal
    URL and the session token so that a check can assert over HTTP where that
    is the honest surface (status codes, JSON shapes) and over the DOM where
    that is (layout, copy, what a button does).
    """

    def __init__(self, portal_url, token, out_dir):
        self.portal_url = portal_url.rstrip("/")
        self.token = token
        self.out_dir = out_dir
        self.fixtures = None  # set by the runner; see fixtures.py

        self._process = None
        self._profile = None
        self._socket = None
        self._next_id = 0
        self._events = []

    # ------------------------------------------------------------ lifecycle

    def start(self):
        if websocket is None:
            raise BrowserMissing(
                "the drive needs the `websocket-client` package for its CDP socket.\n"
                "Install it with:  python3 -m pip install websocket-client"
            )
        binary = find_browser()
        port = free_port()
        self._profile = tempfile.mkdtemp(prefix="semlith-drive-profile-")
        args = [
            binary,
            "--headless=new",
            "--remote-debugging-port=%d" % port,
            "--user-data-dir=%s" % self._profile,
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-gpu",
            "--hide-scrollbars",
            "--window-size=1440,900",
            # The portal is served over plain HTTP on loopback, and Chrome is
            # otherwise happy to upgrade or block parts of it.
            "--disable-features=Translate,MediaRouter",
            "about:blank",
        ]
        self._process = subprocess.Popen(
            args, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
        )

        target = self._wait_for_target(port)
        # `suppress_origin` rather than launching the browser with
        # `--remote-allow-origins=*`: Chrome rejects a debugging socket that
        # arrives with an Origin it was not told to expect, and not sending one
        # is the answer that does not loosen the browser to do it.
        self._socket = websocket.create_connection(target, timeout=30, suppress_origin=True)

        # Enabled once, here, so console errors are collected from the first
        # navigation onwards rather than from whenever a check thinks to ask.
        self._send("Page.enable")
        self._send("Runtime.enable")
        self._send("Log.enable")

    def _wait_for_target(self, port, timeout=30):
        """The WebSocket URL of the browser's first page target."""
        deadline = time.time() + timeout
        last_error = None
        while time.time() < deadline:
            if self._process.poll() is not None:
                raise BrowserMissing(
                    "the browser exited immediately with status %d. Try running it by "
                    "hand to see why." % self._process.returncode
                )
            try:
                raw = urllib.request.urlopen(
                    "http://127.0.0.1:%d/json/list" % port, timeout=2
                ).read()
                for entry in json.loads(raw):
                    if entry.get("type") == "page" and entry.get("webSocketDebuggerUrl"):
                        return entry["webSocketDebuggerUrl"]
            except Exception as error:  # the port is not up yet, usually
                last_error = error
            time.sleep(0.2)
        raise BrowserMissing(
            "the browser never opened a debugging port on %d (last error: %s)"
            % (port, last_error)
        )

    def stop(self):
        if self._socket is not None:
            try:
                self._socket.close()
            except Exception:
                pass
            self._socket = None
        if self._process is not None:
            self._process.terminate()
            try:
                self._process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self._process.kill()
            self._process = None
        if self._profile is not None:
            shutil.rmtree(self._profile, ignore_errors=True)
            self._profile = None

    # -------------------------------------------------------------- protocol

    def _send(self, method, params=None, timeout=60):
        self._next_id += 1
        message_id = self._next_id
        self._socket.send(
            json.dumps({"id": message_id, "method": method, "params": params or {}})
        )
        deadline = time.time() + timeout
        while time.time() < deadline:
            self._socket.settimeout(max(1.0, deadline - time.time()))
            raw = self._socket.recv()
            message = json.loads(raw)
            if message.get("id") == message_id:
                if "error" in message:
                    raise ProtocolError(
                        "%s failed: %s" % (method, message["error"].get("message"))
                    )
                return message.get("result", {})
            if "method" in message:
                self._events.append(message)
        raise ProtocolError("%s did not answer within %ds" % (method, timeout))

    def _drain(self, seconds=0.3):
        """Collect whatever events are already waiting on the socket.

        Events only reach us while something is reading, and the checks read
        constantly, so this only matters just before `console_errors`.
        """
        deadline = time.time() + seconds
        while time.time() < deadline:
            try:
                self._socket.settimeout(max(0.05, deadline - time.time()))
                message = json.loads(self._socket.recv())
            except Exception:
                return
            if "method" in message:
                self._events.append(message)

    # --------------------------------------------------------------- driving

    def navigate(self, url, timeout=30):
        self._send("Page.navigate", {"url": url})
        self.wait_for("document.readyState === 'complete'", timeout=timeout)

    def eval(self, js, timeout=60):
        """Run JavaScript in the page and return the value as JSON.

        The expression is wrapped so a bare statement sequence still works, and
        a promise is awaited, which is what most of the portal's own helpers
        return.
        """
        result = self._send(
            "Runtime.evaluate",
            {
                "expression": js,
                "returnByValue": True,
                "awaitPromise": True,
                "userGesture": True,
            },
            timeout=timeout,
        )
        if "exceptionDetails" in result:
            details = result["exceptionDetails"]
            description = (details.get("exception") or {}).get("description")
            raise ProtocolError(
                "the page threw while evaluating:\n  %s\n%s"
                % (js.strip().splitlines()[0], description or details.get("text"))
            )
        return result.get("result", {}).get("value")

    def click(self, selector):
        """Click the first element matching a CSS selector.

        A real click event through the element rather than a synthetic mouse
        press at coordinates: the portal builds its own controls and listens
        for `click`, and a coordinate press adds a class of flake (scroll
        position, overlay, device pixel ratio) that this drive has no use for.
        """
        found = self.eval(
            """
            (() => {
              const el = document.querySelector(%s);
              if (!el) return false;
              el.scrollIntoView({block: "center"});
              el.click();
              return true;
            })()
            """
            % json.dumps(selector)
        )
        if not found:
            raise ProtocolError(
                "nothing matched %r, so there was nothing to click. The portal's "
                "markup may have moved; update the check that asked for it." % selector
            )

    def click_text(self, selector, text):
        """Click the first element matching `selector` whose text is `text`.

        Menus, chips and confirm dialogs are all addressed by their words in
        this drive, because their words are what the findings document quotes.
        """
        found = self.eval(
            """
            (() => {
              const wanted = %s.trim().toLowerCase();
              for (const el of document.querySelectorAll(%s)) {
                // Only what a person could click. `innerText` on an element
                // that is not rendered falls back to its text content in
                // Chrome, so a `hidden` button still reads as its own label —
                // which had this clicking the Remove on a *live* run card,
                // where it is present and folded away, and reporting the 409
                // the daemon rightly answered with.
                if (el.offsetParent === null && getComputedStyle(el).position !== "fixed") continue;
                if ((el.innerText || el.value || "").trim().toLowerCase() === wanted) {
                  el.scrollIntoView({block: "center"});
                  el.click();
                  return true;
                }
              }
              return false;
            })()
            """
            % (json.dumps(text), json.dumps(selector))
        )
        if not found:
            raise ProtocolError(
                "no %r element a person could click reads %r, so there was "
                "nothing to click." % (selector, text)
            )

    def type(self, selector, text):
        """Focus an element and type into it, as a person would."""
        focused = self.eval(
            """
            (() => {
              const el = document.querySelector(%s);
              if (!el) return false;
              el.scrollIntoView({block: "center"});
              el.focus();
              if ("value" in el) el.value = "";
              return true;
            })()
            """
            % json.dumps(selector)
        )
        if not focused:
            raise ProtocolError("nothing matched %r, so there was nothing to type into." % selector)
        self._send("Input.insertText", {"text": text})
        # `insertText` does not fire the events a hand-rolled control listens
        # for, so they are dispatched here rather than in every check.
        self.eval(
            """
            (() => {
              const el = document.querySelector(%s);
              el.dispatchEvent(new Event("input", {bubbles: true}));
              el.dispatchEvent(new Event("change", {bubbles: true}));
            })()
            """
            % json.dumps(selector)
        )

    KEYS = {
        "Enter": {"key": "Enter", "code": "Enter", "windowsVirtualKeyCode": 13, "text": "\r"},
        "Tab": {"key": "Tab", "code": "Tab", "windowsVirtualKeyCode": 9},
        "Escape": {"key": "Escape", "code": "Escape", "windowsVirtualKeyCode": 27},
        "Backspace": {"key": "Backspace", "code": "Backspace", "windowsVirtualKeyCode": 8},
    }

    def press(self, key):
        if key not in self.KEYS:
            raise ProtocolError(
                "the drive does not know the key %r. Add it to cdp.Drive.KEYS." % key
            )
        spec = self.KEYS[key]
        self._send("Input.dispatchKeyEvent", dict(spec, type="rawKeyDown"))
        if "text" in spec:
            self._send("Input.dispatchKeyEvent", dict(spec, type="char"))
        self._send("Input.dispatchKeyEvent", dict(spec, type="keyUp"))

    def wait_for(self, js_predicate, timeout=20, what=None):
        """Poll a JavaScript predicate until it is truthy.

        Raises rather than returning false, because every caller in this drive
        treats a timeout as a failure and a helper that returns a boolean just
        moves the error message somewhere less useful.
        """
        deadline = time.time() + timeout
        last = None
        while time.time() < deadline:
            try:
                last = self.eval(js_predicate)
            except ProtocolError as error:
                # A predicate that throws has not come true — it has usually
                # dereferenced a node that is not on the page yet, which is the
                # exact state this is waiting out. Recorded for the timeout
                # message and then polled again: returning the exception's text
                # here made every throwing predicate succeed on its first poll,
                # because a non-empty string is truthy.
                last = "threw: %s" % error
                time.sleep(0.1)
                continue
            if last:
                return last
            time.sleep(0.1)
        raise ProtocolError(
            "waited %ds for %s and it never became true (last value: %r)"
            % (timeout, what or js_predicate.strip().splitlines()[0], last)
        )

    def screenshot(self, path):
        result = self._send("Page.captureScreenshot", {"format": "png"})
        with open(path, "wb") as handle:
            handle.write(base64.b64decode(result["data"]))

    def shot(self, name):
        """A screenshot into the drive's output directory, by bare name."""
        if not name.endswith(".png"):
            name += ".png"
        path = os.path.join(self.out_dir, name)
        self.screenshot(path)
        return path

    def console_errors(self):
        """Every console and log entry at error level since the last clear."""
        self._drain()
        errors = []
        for event in self._events:
            method = event.get("method")
            params = event.get("params", {})
            if method == "Runtime.consoleAPICalled" and params.get("type") == "error":
                parts = [
                    str(arg.get("value", arg.get("description", "")))
                    for arg in params.get("args", [])
                ]
                errors.append(" ".join(parts).strip())
            elif method == "Log.entryAdded":
                entry = params.get("entry", {})
                if entry.get("level") == "error":
                    text = entry.get("text", "")
                    url = entry.get("url")
                    errors.append("%s %s" % (text, url) if url else text)
        return errors

    def clear_console(self):
        self._drain()
        self._events = []

    def set_viewport(self, width, height, mobile=False):
        self._send(
            "Emulation.setDeviceMetricsOverride",
            {
                "width": width,
                "height": height,
                "deviceScaleFactor": 1,
                "mobile": mobile,
            },
        )

    def reset_viewport(self):
        self._send("Emulation.clearDeviceMetricsOverride")

    # ------------------------------------------------------- portal helpers

    # Every view in `src/portal/app.js` is an `async` function that awaits one
    # or more `/api/…` calls before it returns a single node, and `render()`
    # fills the page with a bare "Loading…" while it does. So a check that
    # navigates and then reads immediately reads the *previous* view — or an
    # empty one — and reports the control it wanted as missing.
    #
    # The heading is the signal that the awaits are over: `render()` picks the
    # view out of `VIEWS`, and each view's own `pageHead(title, …)` writes that
    # same title into the one `h1` on the page. The Search page's is
    # `el("h1", {class: "sr-only", text: "Search"})`, which is the same
    # contract. Until the awaited node is inserted there is no `h1` at all.
    #
    #: view id -> the `h1` that view paints, from `VIEWS` in `src/portal/app.js`.
    VIEW_TITLES = {
        "stores": "Stores",
        "files": "Files",
        "index": "Inside the index",
        "search": "Search",
        "graph": "Graph",
        "impact": "Impact",
        "agents": "Agents",
        "ledger": "Retrieval ledger",
        "reports": "Reports",
        "cloud": "Cloud",
        "privacy": "Privacy",
        "doctor": "Doctor",
        "about": "About",
    }

    #: The views whose heading paints before their content does, and the
    #: expression that is true once the content is there too. The Graph page is
    #: the only one: it returns its frame and then fetches `/api/graph` from a
    #: `setTimeout`, because the canvas has no size until it is in the document,
    #: so its heading is on screen while `.graph-count` still reads "loading…".
    VIEW_READY = {
        "graph": (
            "/\\d[\\d,]*\\s*symbols/i.test("
            "((document.querySelector('.graph-count') || {}).innerText) || '')"
        ),
    }

    def open_view(self, view_id, fresh=False, timeout=30):
        """Open one of the portal's views and wait until it is actually on screen.

        The token travels in the query on the first load only; the page moves
        it into sessionStorage and takes it out of the address bar, exactly as
        `src/portal/app.js` describes. `fresh=True` forces the query form
        again, which is what a check wants when it has just cleared storage.
        """
        if view_id not in self.VIEW_TITLES:
            raise ProtocolError(
                "the portal has no view called %r. The thirteen are %s."
                % (view_id, ", ".join(sorted(self.VIEW_TITLES)))
            )

        already_loaded = False
        if not fresh:
            try:
                already_loaded = bool(self.eval("!!document.querySelector('#root nav')"))
            except Exception:
                already_loaded = False

        if already_loaded:
            self.eval("location.hash = %s" % json.dumps("#" + view_id))
        else:
            self.navigate("%s/?token=%s#%s" % (self.portal_url, self.token, view_id))

        title = self.VIEW_TITLES[view_id]
        self.wait_for(
            "(() => { const h = document.querySelector('#root h1');"
            " return !!h && (h.textContent || '').trim() === %s; })()" % json.dumps(title),
            timeout=timeout,
            what=(
                "the #%s view's own heading to read %r. Every view awaits an "
                "/api/… call before anything of it is inserted, so a view that "
                "has merely been navigated to is still the one before it — and "
                "with no store open the whole page is the welcome screen, whose "
                "heading is 'No stores yet'." % (view_id, title)
            ),
        )
        extra = self.VIEW_READY.get(view_id)
        if extra:
            self.wait_for(
                extra,
                timeout=timeout,
                what="the #%s view's content to arrive behind its heading" % view_id,
            )
        # One frame of settle, so a check reading geometry does not read it
        # mid-render. Cheap, and it removes a whole class of flake.
        self.eval("new Promise(done => requestAnimationFrame(() => done(true)))")

    # The Index page keeps its folder picker, its projects checklist, its URL
    # panel and its machine limits behind one button each, and the four are
    # mutually exclusive: opening one closes the rest. Every panel starts
    # `hidden`, and a hidden subtree contributes nothing to `innerText`, so a
    # check that reads a panel without pressing its button reads an empty
    # string and blames the product for a control that is simply folded away.
    #
    # The value is a JavaScript expression that is true once the panel that
    # button reveals is actually on screen. Two of the buttons open a picker,
    # and only one picker can be open at a time, which is why they share one.
    INDEX_PANELS = {
        "Choose folders…": (
            "!!document.querySelector('.card.picker:not([hidden]) .crumbs')"
        ),
        "Projects under a folder…": (
            "!!document.querySelector('.card.picker:not([hidden]) .crumbs')"
        ),
        "Add from a URL": (
            "(() => { const field = document.querySelector('#index-url');"
            " return !!field && !field.closest('.card').hidden; })()"
        ),
        "Machine limits": (
            "(() => { const field = document.querySelector("
            "'input[aria-label=\"runs at once\"]');"
            " return !!field && !field.closest('.card').hidden; })()"
        ),
    }

    def _index_panel_button(self, label, timeout=20):
        """The `aria-pressed` of the Index page button reading `label`.

        Waited for rather than read once. `indexView()` awaits `refreshStores()`
        and `refreshRuns()` before any of its four buttons exists, so a single
        miss means "not yet", not "gone" — and reporting it as gone is what made
        five checks blame the product for a button that arrived a moment later.
        Both answers, `"true"` and `"false"`, are non-empty strings and so are
        truthy to `wait_for`; only the absent button polls again.
        """
        if label not in self.INDEX_PANELS:
            raise ProtocolError(
                "the drive does not know an Index page panel called %r. The four "
                "are %s." % (label, ", ".join(sorted(self.INDEX_PANELS)))
            )
        return self.wait_for(
            """
            (() => {
              const wanted = %s.trim().toLowerCase();
              for (const button of document.querySelectorAll('button.secondary')) {
                if ((button.innerText || '').trim().toLowerCase() === wanted) {
                  return button.getAttribute('aria-pressed') || 'false';
                }
              }
              return null;
            })()
            """
            % json.dumps(label),
            timeout=timeout,
            what=(
                "the Index page's %r button. The page's four reveal buttons are "
                "how every one of its panels is reached; if their wording moved, "
                "update `INDEX_PANELS` rather than the check." % label
            ),
        )

    def open_index_panel(self, label, timeout=20):
        """Press one of the Index page's four reveal buttons and wait for its panel.

        A panel that is already open is left alone, because pressing its button
        again is what closes it.
        """
        if self._index_panel_button(label) != "true":
            self.click_text("button.secondary", label)
        self.wait_for(
            self.INDEX_PANELS[label], timeout=timeout, what="the %r panel" % label
        )

    def close_index_panel(self, label):
        """Fold a panel back up, so the next check finds the page as it was."""
        if self._index_panel_button(label) == "true":
            self.click_text("button.secondary", label)

    # ---------------------------------------------------------- portal HTTP

    def _request(self, path, method="GET", body=None):
        url = path if path.startswith("http") else self.portal_url + path
        data = None
        headers = {"Semlith-Token": self.token}
        if body is not None:
            data = json.dumps(body).encode("utf-8")
            headers["Content-Type"] = "application/json"
            # The daemon checks fetch metadata before the token on a write, so
            # a request that does not look same-origin is refused with a 403
            # long before the route is reached. See `src/http.rs`.
            headers["Sec-Fetch-Site"] = "same-origin"
            headers["Origin"] = self.portal_url
        request = urllib.request.Request(url, data=data, headers=headers, method=method)
        try:
            with urllib.request.urlopen(request, timeout=120) as response:
                return response.status, response.read()
        except urllib.error.HTTPError as error:
            return error.code, error.read()

    def api(self, path, method="GET", body=None):
        """A portal API call, returning parsed JSON, raising on a non-2xx."""
        status, raw = self._request(path, method, body)
        if status < 200 or status >= 300:
            raise ProtocolError(
                "%s %s answered %d: %s" % (method, path, status, raw.decode("utf-8", "replace")[:400])
            )
        return json.loads(raw.decode("utf-8"))

    def api_result(self, path, method="GET", body=None):
        """The status and the parsed body, for checks that assert on both."""
        status, raw = self._request(path, method, body)
        try:
            parsed = json.loads(raw.decode("utf-8"))
        except ValueError:
            parsed = raw.decode("utf-8", "replace")
        return status, parsed

    def status(self, path, with_token=True):
        """The status code of a bare GET, optionally without the token."""
        url = path if path.startswith("http") else self.portal_url + path
        headers = {"Semlith-Token": self.token} if with_token else {}
        request = urllib.request.Request(url, headers=headers, method="GET")
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                return response.status
        except urllib.error.HTTPError as error:
            return error.code
        except urllib.error.URLError as error:
            raise ProtocolError("GET %s could not be made at all: %s" % (path, error))


if __name__ == "__main__":
    # A self-check: launch a browser, load a data URL, read it back. It proves
    # the socket, the evaluate path and the screenshot path without needing a
    # daemon, which is what you want when the drive will not start and you do
    # not yet know whose fault it is.
    scratch = tempfile.mkdtemp(prefix="semlith-cdp-selfcheck-")
    drive = Drive("http://127.0.0.1:7365", "unused", scratch)
    drive.start()
    try:
        drive.navigate("data:text/html,<h1 id=t>hello</h1>")
        text = drive.eval("document.getElementById('t').innerText")
        if text != "hello":
            raise SystemExit("expected 'hello' from the page, got %r" % text)
        drive.set_viewport(390, 844)
        width = drive.eval("window.innerWidth")
        if width != 390:
            raise SystemExit("expected a 390px viewport, got %r" % width)
        drive.shot("cdp-selfcheck")
        print("cdp.py self-check passed (%s)" % find_browser())
    finally:
        drive.stop()
        shutil.rmtree(scratch, ignore_errors=True)
    sys.exit(0)
