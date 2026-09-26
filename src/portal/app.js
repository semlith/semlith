/* semlith portal
 *
 * Plain DOM, no framework, no build step. The whole file is served from the
 * binary, so there is nothing to bundle and nothing to fetch: what you read
 * here is what runs.
 *
 * Every view is a function that returns an element, and the router swaps one
 * for another inside <main>. The shell around it — topbar, rail, drawer — is
 * built once and then mutated in place. That split matters: rebuilding the
 * shell to open a menu used to re-run the current view, which re-hit the API
 * and threw away whatever the user had typed into it.
 */

"use strict";

// ---------------------------------------------------------------- helpers

/** Build an element. Attributes in `props`, children as the rest. */
function el(tag, props, ...kids) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(props || {})) {
    if (value === null || value === undefined || value === false) continue;
    if (key === "class") node.className = value;
    else if (key === "text") node.textContent = value;
    else if (key.startsWith("on")) node.addEventListener(key.slice(2), value);
    else node.setAttribute(key, value === true ? "" : String(value));
  }
  for (const kid of kids.flat()) {
    if (kid === null || kid === undefined || kid === false) continue;
    node.append(kid.nodeType ? kid : document.createTextNode(String(kid)));
  }
  return node;
}

/** Append children to an element and return the element, for use inline. */
function appended(node, ...kids) {
  for (const kid of kids.flat()) if (kid) node.append(kid);
  return node;
}

/* What a page shows while its first request is in flight.
 *
 * The mark is a four-by-four grid of cells, five of them amber, so the wait
 * is the mark assembling itself rather than the word "Loading…" on an empty
 * page. Sixteen spans and one keyframe: no image to fetch, nothing to
 * animate in JavaScript, and it inherits the theme because the cells are
 * painted with the same two tokens everything else uses.
 *
 * The cells are indexed so the stylesheet can stagger them along the
 * diagonal, and the five the real mark paints amber carry `hot`, so what
 * assembles is this product's mark and not a generic spinner.
 */
const LOADER_HOT = new Set([1, 6, 7, 9, 14]);

function loadingView(what) {
  const cells = [];
  for (let i = 0; i < 16; i++) {
    cells.push(el("span", { class: LOADER_HOT.has(i) ? "cell hot" : "cell", "data-i": String(i) }));
  }
  return el(
    "div",
    { class: "view loading", role: "status", "aria-live": "polite" },
    el("div", { class: "mark", "aria-hidden": "true" }, cells),
    el("span", { class: "what", text: what ? `Reading ${what.toLowerCase()}…` : "Reading…" }),
    el("span", { class: "track" }, el("span", { class: "run" })),
  );
}

/* The second stage of a wait: the shape of what is coming.
 *
 * The page-wide loader covers the view's own request. A panel that fetches
 * after the page has drawn — the graph's health cards, the map, the agents
 * list — is a second wait, and it used to be a line of grey text where a
 * card was about to be. A skeleton says how much is coming and stops the
 * layout jumping when it lands.
 *
 * `widths` are percentages, one per line, so a skeleton of a list of names
 * does not look like a skeleton of a paragraph.
 */
function skeleton(...widths) {
  return el(
    "div",
    { class: "skel", "aria-hidden": "true" },
    widths.map((width) => {
      const line = el("span", { class: "line" });
      line.style.width = `${width}%`;
      return line;
    }),
  );
}

/** `rows` skeleton lines of alternating length, for a list of unknown size. */
function skeletonRows(rows) {
  const widths = [];
  for (let i = 0; i < rows; i++) widths.push([92, 78, 85, 64][i % 4]);
  return skeleton(...widths);
}

/** Replace an element's children. */
function fill(node, ...kids) {
  node.replaceChildren();
  for (const kid of kids.flat()) {
    if (kid === null || kid === undefined || kid === false) continue;
    node.append(kid.nodeType ? kid : document.createTextNode(String(kid)));
  }
  return node;
}

/* Write words into a node that a live poll keeps rewriting.
 *
 * `textContent =` swaps the node's text child for a new one even when the words
 * are the same, so a card written once a second replaced a child per field per
 * second — which a live region re-announces, a selection loses and a
 * MutationObserver reports as churn. This edits the one text node's data, and
 * only when the words moved. A bare Text node works too, for the words inside
 * a pill that sit beside its dot. */
function setText(node, value) {
  const text = value === null || value === undefined ? "" : String(value);
  if (node.nodeType === Node.TEXT_NODE) {
    if (node.data !== text) node.data = text;
    return node;
  }
  const only = node.firstChild;
  if (only && only === node.lastChild && only.nodeType === Node.TEXT_NODE) {
    if (only.data !== text) only.data = text;
  } else if (node.textContent !== text) {
    node.textContent = text;
  }
  return node;
}

/* Put a parent's children into `want`'s order, moving only what is out of place.
 *
 * Appending every child in turn re-parents all of them on every call, and a
 * node that is taken out and put back loses its focus and the scroll position
 * of everything inside it. The Index page did that to every run card once a
 * second, so a focused Pause button was blurred before anyone could press it
 * twice and a log scrolled back to read was thrown to its top.
 *
 * What stays is the longest run of children already in the wanted order, and
 * only the rest move — so one card finishing and dropping below the live ones
 * is one move, not one for every card it passes. The focused node is always
 * among those that stay. A node not yet in `parent` is inserted where it
 * belongs, which is an insertion and not a move. A node named twice is placed
 * once: two answers can name one run while a store is being opened. */
function arrange(parent, want) {
  const order = [...new Set(want.filter(Boolean))];
  const rank = new Map(order.map((node, i) => [node, i]));
  const present = [...parent.children].filter((child) => rank.has(child));
  const focused = present.findIndex((child) => child.contains(document.activeElement));
  const items = present.map((child, at) => ({ at, value: rank.get(child) }));
  const pivot = focused >= 0 ? items[focused].value : -1;
  const staying =
    focused < 0
      ? increasing(items)
      : [
          ...increasing(items.filter((item) => item.at < focused && item.value < pivot)),
          focused,
          ...increasing(items.filter((item) => item.at > focused && item.value > pivot)),
        ];
  const stays = new Set(staying.map((at) => present[at]));
  let next = null;
  for (let i = order.length - 1; i >= 0; i--) {
    const node = order[i];
    const placed =
      node.parentNode === parent && (next ? node.nextElementSibling === next : !node.nextElementSibling);
    if (!stays.has(node) && !placed) parent.insertBefore(node, next);
    next = node;
  }
}

/** The `at`s of a longest strictly increasing run of `value`s, in order. */
function increasing(items) {
  const tails = [];
  const before = [];
  items.forEach((item, k) => {
    let lo = 0;
    let hi = tails.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (items[tails[mid]].value < item.value) lo = mid + 1;
      else hi = mid;
    }
    before[k] = lo > 0 ? tails[lo - 1] : -1;
    tails[lo] = k;
  });
  const out = [];
  for (let k = tails.length ? tails[tails.length - 1] : -1; k >= 0; k = before[k]) out.unshift(items[k].at);
  return out;
}

/* The session token.
 *
 * It arrives once, in the query of the URL the daemon printed, and from then on
 * it lives in this page and in sessionStorage — never in a cookie. A cookie
 * would be attached to a request any page on any other 127.0.0.1 port made,
 * because every port on localhost is the same site; a header is attached only
 * by this page.
 *
 * The token is taken out of the address bar as soon as it is read, so it is not
 * in the history, not in a bookmark, and not in what someone screenshots. The
 * reload path is why sessionStorage is here at all: refreshing the page sends a
 * request this script did not make, so the URL it reloads must not need to
 * carry the token.
 */
const TOKEN_HEADER = "Semlith-Token";

const session = (() => {
  const KEY = "semlith.token";
  let token = "";
  try {
    const fromUrl = new URLSearchParams(location.search).get("token");
    if (fromUrl) {
      token = fromUrl;
      sessionStorage.setItem(KEY, token);
      const clean = location.pathname + location.hash;
      history.replaceState(null, "", clean || "/");
    } else {
      token = sessionStorage.getItem(KEY) || "";
    }
  } catch (_) {
    /* A browser with storage disabled still works for as long as this document
     * lives; only the reload stops surviving. */
  }
  return {
    get: () => token,
    set(fresh) {
      token = fresh;
      try {
        sessionStorage.setItem(KEY, fresh);
      } catch (_) {
        /* as above */
      }
    },
  };
})();

/** The headers every request to this daemon carries. */
function authed(extra) {
  return { ...(extra || {}), [TOKEN_HEADER]: session.get() };
}

/* An image the store indexed, fetched rather than linked.
 *
 * A browser attaches no header to an `<img src>`, and since 0.14.0 the token is
 * a header, so the bytes are read through `fetch` and handed to the element
 * directly.
 *
 * A `data:` URL rather than an object URL, which is what this used until
 * 0.16.0 and why no preview ever appeared: the policy the server sends is
 * `img-src 'self' data:`, and a `blob:` URL is neither, so every one of them
 * was refused before it decoded. `data:` costs the base64 third and has no
 * handle to revoke — acceptable because the body panel holds one picture at a
 * time. Widening the policy to `blob:` would have been the other fix, and the
 * policy is the thing this product is checkable on. */
function imagePreview(path) {
  const img = el("img", { class: "preview", alt: "" });
  const failed = () =>
    img.replaceWith(el("pre", { class: "muted", text: `${path} could not be read` }));
  fetch(`/api/image?path=${encodeURIComponent(path)}`, {
    credentials: "omit",
    headers: authed(),
  })
    .then((response) => (response.ok ? response.blob() : Promise.reject(response.statusText)))
    .then((blob) => {
      const reader = new FileReader();
      reader.onload = () => {
        img.src = String(reader.result);
      };
      reader.onerror = failed;
      reader.readAsDataURL(blob);
    })
    .catch(failed);
  return img;
}

async function api(path, options) {
  const options_ = options || {};
  const response = await fetch(path, {
    // No cookie is sent because there is none to send, and saying so keeps a
    // future one from being attached by accident.
    credentials: "omit",
    ...options_,
    headers: authed(options_.headers),
  });
  if (!response.ok) {
    let detail = response.statusText;
    try {
      const body = await response.json();
      if (body && body.error) detail = body.error;
    } catch (_) {
      /* an empty body is what 401 sends on purpose */
    }
    throw new Error(detail || "request failed");
  }
  return response.json();
}

function post(path, body) {
  return api(path, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
}

const NUM = new Intl.NumberFormat();
const n = (value) => NUM.format(value || 0);

/** `part` as a percentage of `whole`, for a bar's own reading of itself.
 *
 * One decimal under 10%, none above: `0.4%` and `62%` are both the precision
 * a reader can use, and `0%` beside a visible sliver reads as a bug. */
function share(part, whole) {
  if (!whole) return "0%";
  const pct = (part / whole) * 100;
  return `${pct < 10 ? pct.toFixed(1) : Math.round(pct)}%`;
}

function bytes(value) {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let size = Number(value) || 0;
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) {
    size /= 1024;
    unit += 1;
  }
  return `${unit === 0 ? size : size.toFixed(1)} ${units[unit]}`;
}

/** The offset this machine is on, as `+05:30`, for a clock that says so. */
function zone() {
  // `getTimezoneOffset` is minutes *behind* UTC, so its sign is the opposite
  // of the one written in a timestamp.
  const minutes = -new Date().getTimezoneOffset();
  const sign = minutes < 0 ? "-" : "+";
  const off = Math.abs(minutes);
  return `${sign}${String(Math.floor(off / 60)).padStart(2, "0")}:${String(off % 60).padStart(2, "0")}`;
}

function when(unix) {
  if (!unix) return "never";
  const seconds = Math.max(0, Math.floor(Date.now() / 1000) - unix);
  if (seconds < 60) return `${seconds}s ago`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
  return `${Math.floor(seconds / 86400)}d ago`;
}

function clock(unix) {
  // With the offset. This printed the browser's local time while `semlith
  // ledger` printed UTC, with neither saying which — so a portal event and a
  // ledger row for the same moment were hours apart and nothing admitted it.
  return `${new Date(unix * 1000).toTimeString().slice(0, 8)} ${zone()}`;
}

const SVG_NS = "http://www.w3.org/2000/svg";

/** An inline icon. Decorative: the control around it carries the name. */
function icon(d, size, solid) {
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("width", String(size || 16));
  svg.setAttribute("height", String(size || 16));
  // A shape that is a shape rather than a stroke — the three dots of a menu
  // control. Drawn as strokes they are zero-length segments whose whole size
  // is the line width, which is as faint as a mark can be.
  svg.setAttribute("fill", solid ? "currentColor" : "none");
  svg.setAttribute("stroke", solid ? "none" : "currentColor");
  svg.setAttribute("stroke-width", "1.7");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("aria-hidden", "true");
  for (const segment of String(d).split("|")) {
    const path = document.createElementNS(SVG_NS, "path");
    path.setAttribute("d", segment);
    svg.append(path);
  }
  return svg;
}

const ICONS = {
  menu: "M4 7h16|M4 12h16|M4 17h16",
  search: "M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14z|M20 20l-3.5-3.5",
  plus: "M12 5v14|M5 12h14",
  sun: "M12 3v2|M12 19v2|M3 12h2|M19 12h2|M5.6 5.6 7 7|M17 17l1.4 1.4|M18.4 5.6 17 7|M7 17l-1.4 1.4|M12 7.8a4.2 4.2 0 1 0 0 8.4 4.2 4.2 0 0 0 0-8.4z",
  folder: "M3 7.5A1.5 1.5 0 0 1 4.5 6h4l2 2.5h7A1.5 1.5 0 0 1 19 10v7a1.5 1.5 0 0 1-1.5 1.5h-13A1.5 1.5 0 0 1 3 17z",
  file: "M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z|M14 3v5h5",
  up: "M5 12h14|M11 6l-6 6 6 6",
  // `up` reversed: the mark the design puts on a link that carries you on to
  // the next page rather than back to the last one.
  arrowRight: "M5 12h14|M13 6l6 6-6 6",
  alert: "M12 9v4|M12 17h.01|M12 4 3 19h18z",
  monitor: "M4 5h16v10H4z|M9 19h6|M12 15v4",
  moon: "M20 14.5A8.5 8.5 0 0 1 9.5 4a8.5 8.5 0 1 0 10.5 10.5z",
  copy: "M9 9h9a1 1 0 0 1 1 1v9a1 1 0 0 1-1 1H9a1 1 0 0 1-1-1v-9a1 1 0 0 1 1-1z|M6 15H5a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1h9a1 1 0 0 1 1 1v1",
  // Three filled dots, as circles rather than as dotted strokes.
  more:
    "M12 4.1a2 2 0 1 0 0 4 2 2 0 0 0 0-4z|M12 10a2 2 0 1 0 0 4 2 2 0 0 0 0-4z|"
    + "M12 15.9a2 2 0 1 0 0 4 2 2 0 0 0 0-4z",
  // Three tracks with a handle on each: the machine's three numbers.
  sliders: "M4 7h10|M18 7h2|M4 12h4|M12 12h8|M4 17h11|M19 17h1|M15 5v4|M9 10v4|M16 15v4",
  // The explorer tree's marks: a disclosure chevron, and a folder drawn open.
  chevron: "M9 6l6 6-6 6",
  folderOpen: "M3 7.5A1.5 1.5 0 0 1 4.5 6h4l2 2.5h7A1.5 1.5 0 0 1 19 10v1H7.2a1.5 1.5 0 0 0-1.4 1L3.6 18.5|M3.6 18.5 6 12h15l-2.4 6.5z",
};

/* The nav marks, from the design. `|` separates subpaths so one mark can be
 * more than a single stroke. */

const NAV_ICONS = {
  stores: "M12 4l8 4-8 4-8-4 8-4|M4 12l8 4 8-4|M4 16.5l8 4 8-4",
  files: "M6 3h7l5 5v13H6z|M13 3v5h5",
  index: "M4 6h16|M4 12h10|M4 18h13",
  // The design's own mark for this page: two book spines. What is already on
  // the shelf, as against `index`, which is the list of what to put there.
  corpus: "M5 4h6v16H5z|M13 4h6v16h-6",
  search: "M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14z|M16.2 16.2 20 20",
  graph: "M5 6h4v4H5z|M15 14h4v4h-4z|M9 8h4v8h2",
  // v3's mark: a plain ruled page. The folded corner it grew in v4 reads as a
  // document you were handed rather than as a record this machine keeps.
  ledger: "M5 4h14v16H5z|M8 9h8|M8 13h8|M8 17h5",
  agents: "M9 3h6v5H9z|M12 8v3|M5 11h14v9H5z|M9 15h.01|M15 15h.01",
  privacy: "M12 3l7 3v6c0 4.3-3 7.3-7 9-4-1.7-7-4.7-7-9V6z",
  // A trace with a beat in it: this page is a reading of the machine, and the
  // rail is icons only — a nav item that rendered nothing was the one item
  // with no way to tell what it was.
  doctor: "M3 12h3l2-5 3 10 2.5-7 1.5 2h6",
  about: "M12 4a8 8 0 1 0 0 16 8 8 0 0 0 0-16z|M12 11v5|M12 8h.01",
  // v3's mark, back: a line climbing to a point, with the axis it climbs to.
  impact: "M4 18l5-6 4 3 7-9|M20 6h-4|M20 6v4",
  reports: "M6 3h8l4 4v14H6z|M14 3v4h4|M9 17v-3|M12 17v-6|M15 17v-4",
  cloud: "M7.5 18a4 4 0 0 1 .3-8A5 5 0 0 1 17 9.6 3.6 3.6 0 0 1 16.5 18z",
};

/** A button that copies text and says so for a moment.
 *
 * Invisible until the pointer enters the block it belongs to — see `.copy` in
 * the stylesheet — so a page never shows a column of copy buttons competing
 * with the content they copy. */
function copyButton(getText, label, word) {
  const was = label || "Copy";
  // A field wide enough to hold a command has room for the word; the icon is
  // for a code block, where the control sits over the text it copies.
  const face = () => (word ? was : icon(ICONS.copy, 14));
  const button = el("button", {
    class: "button secondary small copy",
    type: "button",
    "aria-label": was,
    title: was,
    onclick: async () => {
      const text = typeof getText === "function" ? getText() : getText;
      try {
        await navigator.clipboard.writeText(text);
      } catch (_) {
        // No clipboard permission, or an insecure context. Selecting the text
        // is still a way to copy it, and silently doing nothing is not.
        const area = el("textarea", { class: "offscreen" });
        area.value = text;
        document.body.append(area);
        area.select();
        try {
          document.execCommand("copy");
        } catch (__) {
          /* nothing else to try */
        }
        area.remove();
      }
      button.classList.add("done");
      fill(button, "Copied");
      setTimeout(() => {
        button.classList.remove("done");
        fill(button, face());
      }, 1400);
    },
  });
  fill(button, face());
  return button;
}

function copyField(command, word) {
  return el(
    "div",
    { class: word ? "copyfield worded" : "copyfield" },
    el("code", { class: "text", text: command }),
    copyButton(command, "Copy", word),
  );
}

/** A code block with its copy control over the top-right corner. */
function codeBlock(text, label) {
  return el(
    "div",
    { class: "code-block" },
    el("pre", { class: "code", text }),
    copyButton(text, label || "Copy"),
  );
}

/* A command, a path or an identifier set inside a sentence.
 *
 * This is what makes the capitalisation rule read as a rule rather than as a
 * typo: the product is "Semlith" in prose and `semlith` is a command, and the
 * only thing that tells a reader which one they are looking at is that one of
 * them is set in mono. */
function mono(text) {
  return el("code", { class: "mono", text });
}

/** A paragraph of prose, where any part may be a mono command. */
function says(...parts) {
  return el("p", { class: "subtitle" }, parts);
}

/** A failure. Coloured like a problem, because it is one. */
function error(message) {
  return el(
    "div",
    { class: "error", role: "alert" },
    icon(ICONS.alert, 17),
    el("span", { class: "what", text: message }),
  );
}

/** An absence. Nothing went wrong; there is simply nothing here. */
function empty(message) {
  return el("div", { class: "empty" }, message);
}

/* ------------------------------------------------------------------- tips */

/* One tooltip for the whole portal.
 *
 * The element lives at the end of <body> and is positioned against the
 * viewport, which is the whole point: the previous implementation was a
 * `::after` on the element being described, and `.table-wrap` carries
 * `overflow: auto`, so a scroll container clipped it. The Files page's path
 * tip — the reason the feature exists — was the one that could not be read.
 *
 * A pointer places it beside the pointer and it follows as the pointer moves,
 * which is how the Graph canvas's card has always behaved and how the design
 * draws every hover: one behaviour everywhere, rather than a card that tracks
 * the mouse on one page and hangs off an element's corner on the next. Focus
 * has no pointer, so a keyboard-shown tip hangs off the focused element
 * instead, above it and flipped below when there is no room. */
const tip = {
  node: null,
  owner: null,

  ensure() {
    if (!this.node) {
      this.node = el("div", { class: "tip", role: "tooltip", "aria-hidden": "true" });
      document.body.append(this.node);
    }
    return this.node;
  },

  /** Fill and open it; the caller places it. */
  open(content, owner) {
    const node = this.ensure();
    this.owner = owner || null;
    fill(node, content);
    node.setAttribute("aria-hidden", "false");
    node.dataset.open = "true";
    return node;
  },

  /** Hang it off a viewport rectangle: `content` is a string or an element. */
  show(content, rect, owner) {
    const node = this.open(content, owner);
    // Measured after filling and before positioning: the size depends on the
    // text, and a stale measurement puts the flip decision on the wrong side.
    // `offset*` rather than the bounding box, which the opening scale shrinks.
    const width = node.offsetWidth;
    const height = node.offsetHeight;
    const margin = 8;
    let left = rect.left;
    if (left + width > window.innerWidth - margin) {
      left = Math.max(margin, window.innerWidth - margin - width);
    }
    let top = rect.top - height - 6;
    // Above by default, below when there is no room — the flip the criterion
    // asks for, and the reason this is measured against the viewport.
    if (top < margin) top = rect.bottom + 6;
    node.style.left = `${Math.max(margin, Math.round(left))}px`;
    node.style.top = `${Math.round(top)}px`;
  },

  /** Keep an open tip beside the pointer: above and to the right of it,
   * flipped to the left or below at the viewport's edge, as the design's
   * `tipMove` places it. */
  follow(x, y) {
    const node = this.node;
    if (!node) return;
    const width = node.offsetWidth;
    const height = node.offsetHeight;
    const margin = 10;
    let left = x + 14;
    let top = y - height - 14;
    if (left + width > window.innerWidth - margin) left = Math.max(margin, x - width - 14);
    if (top < margin) top = y + 20;
    node.style.left = `${Math.round(left)}px`;
    node.style.top = `${Math.round(top)}px`;
  },

  /** What an element's own tip says: a card when it carries one, else text. */
  contentOf(target) {
    return target.hasAttribute("data-tip-title") ? tipCardOf(target) : target.getAttribute("data-tip");
  },

  /** Hang a tip off an element, for focus, which has no pointer to follow. */
  at(target) {
    this.show(this.contentOf(target), target.getBoundingClientRect(), target);
  },

  /** Open a tip beside a point: the canvases, and every pointer hover. */
  atPoint(x, y, content, owner) {
    this.open(content, owner);
    this.follow(x, y);
  },

  hide(owner) {
    if (!this.node) return;
    if (owner && this.owner !== owner) return;
    this.node.dataset.open = "false";
    this.node.setAttribute("aria-hidden", "true");
    this.owner = null;
  },
};

/* Delegated, so a tip costs nothing per row: every table in the portal renders
 * its rows fresh, and binding two listeners to each of a thousand cells is how
 * a scroll starts to stutter. */
function wireTips() {
  const find = (node) => (node && node.closest ? node.closest("[data-tip], [data-tip-title]") : null);

  document.addEventListener("pointerover", (e) => {
    const target = find(e.target);
    if (target) {
      // Moving between an element's own children fires this again; the tip
      // is already showing, and refilling it would only reset its fade.
      if (tip.owner !== target) tip.atPoint(e.clientX, e.clientY, tip.contentOf(target), target);
    } else if (tip.owner && tip.owner.nodeType) tip.hide();
  });
  // Follows the pointer across the element it describes, as the canvas's does.
  document.addEventListener("pointermove", (e) => {
    if (tip.owner && tip.owner.nodeType && tip.owner.contains(e.target)) tip.follow(e.clientX, e.clientY);
  });
  document.addEventListener("pointerout", (e) => {
    const target = find(e.target);
    // Into one of its own children is not leaving it.
    if (target && !(e.relatedTarget && target.contains(e.relatedTarget))) tip.hide(target);
  });
  // Keyboard parity: focus shows the same label a hover does.
  document.addEventListener("focusin", (e) => {
    const target = find(e.target);
    if (target) tip.at(target);
  });
  document.addEventListener("focusout", (e) => {
    const target = find(e.target);
    if (target) tip.hide(target);
  });
  // A fixed tip does not travel with the row it describes, so it leaves when
  // the row does. Capture, because the scroll happens inside a card.
  window.addEventListener("scroll", () => tip.hide(), true);
  window.addEventListener("resize", () => tip.hide());
}

/* ----------------------------------------------------------------- table */

/* One table for the whole portal: Stores, Files, Languages, the About model
 * list and the Agents client list all render through this. Five hand-written
 * tables is how one page gains sorting and the other four do not.
 *
 * Columns are `{ key, label, className, sortable, value, render }`. `value`
 * pulls the sort key out of a row; `render` returns the cell's content. A
 * table is client-side by default — its rows are already in hand — and
 * `server: true` hands sorting and paging back to the caller instead, which is
 * what the Files table needs: sorting has to order the whole store rather than
 * the page of it that happens to be loaded.
 */
const PER_PAGE = [5, 10, 25, 50];

/* A table's page, page size and sort, by `spec.remember`, for a page that is
 * redrawn whole on a live update. The Stores page is: every watcher write
 * redraws it, and each redraw put the reader back on page 1 at 5 a page. */
const TABLE_VIEWS = new Map();

function dataTable(spec) {
  const columns = spec.columns;
  const view = TABLE_VIEWS.get(spec.remember) || {
    page: 1,
    perPage: spec.perPage || 5,
    sort: spec.sort || null,
    dir: spec.dir || "asc",
  };
  if (spec.remember) TABLE_VIEWS.set(spec.remember, view);
  let rows = spec.rows || [];
  let total = spec.total === undefined ? rows.length : spec.total;

  const headRow = el("tr", {});
  const body = el("tbody", {});
  const foot = el("div", { class: "table-foot" });
  const table = el(
    "table",
    { class: spec.className || null },
    // A caption, always. Four of these tables had neither a caption nor an
    // `aria-label`, so a screen reader announced "table" and left the reader
    // to work out which one from its columns.
    spec.caption ? el("caption", { class: "sr-only", text: spec.caption }) : null,
    el("thead", {}, headRow),
    body,
  );
  // `grow: false` for a table that shares a page with other blocks: stretching
  // a three-row table down a 900px page to fill it is empty space pretending
  // to be a table.
  const node = el(
    "div",
    { class: spec.grow ? "card table-card grow" : "card table-card" },
    el("div", { class: "table-wrap" }, table),
    foot,
  );

  function pages() {
    return Math.max(1, Math.ceil(total / view.perPage));
  }

  function sorted() {
    if (!view.sort) return rows;
    const column = columns.find((c) => c.key === view.sort);
    if (!column || !column.value) return rows;
    const sign = view.dir === "desc" ? -1 : 1;
    // A copy: sorting the caller's array in place would reorder the data
    // behind whatever else is reading it.
    return [...rows].sort((a, b) => {
      // Rows with no value in this column sink, whichever way the sort runs.
      // A column where most rows are blank otherwise hides its own data:
      // sorting the About page's SIZE column ascending put all forty-three
      // dashes first and the five real sizes on the last page.
      if (column.empty) {
        const blank = Number(column.empty(a)) - Number(column.empty(b));
        if (blank) return blank;
      }
      const x = column.value(a);
      const y = column.value(b);
      if (typeof x === "number" && typeof y === "number") return (x - y) * sign;
      return String(x).localeCompare(String(y), undefined, { numeric: true }) * sign;
    });
  }

  function shown() {
    if (spec.server) return rows;
    const from = (view.page - 1) * view.perPage;
    return sorted().slice(from, from + view.perPage);
  }

  function changed() {
    if (spec.server && spec.onChange) spec.onChange({ ...view });
    else paint();
  }

  function sortBy(key) {
    if (view.sort === key) view.dir = view.dir === "asc" ? "desc" : "asc";
    else {
      view.sort = key;
      view.dir = "asc";
    }
    view.page = 1;
    changed();
  }

  function paintHead() {
    fill(
      headRow,
      columns.map((column) => {
        const on = view.sort === column.key;
        if (column.head) {
          // A header that is a control rather than a label — the select-all
          // box above a column of checkboxes.
          return el("th", { class: column.className || null }, column.head());
        }
        if (column.sortable === false) {
          return el("th", { class: column.className || null, text: column.label });
        }
        return el(
          "th",
          {
            class: column.className || null,
            "aria-sort": on ? (view.dir === "asc" ? "ascending" : "descending") : "none",
          },
          el(
            "button",
            { class: "sort", type: "button", onclick: () => sortBy(column.key) },
            column.label,
            el("span", { class: "arrow", text: on ? (view.dir === "asc" ? "↑" : "↓") : "" }),
          ),
        );
      }),
    );
  }

  function paintBody() {
    const page = shown();
    fill(
      body,
      page.map((row, i) =>
        el(
          "tr",
          {},
          columns.map((column) =>
            el(
              "td",
              { class: column.className || null },
              column.render ? column.render(row, i) : String(column.value ? column.value(row) : ""),
            ),
          ),
        ),
      ),
    );
  }

  function paintFoot() {
    const from = total === 0 ? 0 : (view.page - 1) * view.perPage + 1;
    const to = Math.min(total, view.page * view.perPage);
    fill(
      foot,
      el("span", { class: "meta", text: `${n(from)}–${n(to)} of ${n(total)}` }),
      el("span", { class: "divider-v" }),
      el("span", { class: "eyebrow sm", text: "per page" }),
      el(
        "span",
        { class: "chips" },
        PER_PAGE.map((size) =>
          el("button", {
            class: "chip",
            type: "button",
            "aria-pressed": String(size === view.perPage),
            text: String(size),
            onclick: () => {
              view.perPage = size;
              view.page = 1;
              changed();
            },
          }),
        ),
      ),
      el("span", { class: "spacer" }),
      el("button", {
        class: "button secondary small",
        type: "button",
        text: "Prev",
        disabled: view.page <= 1,
        onclick: () => {
          view.page -= 1;
          changed();
        },
      }),
      el("span", { class: "meta", text: `Page ${n(view.page)} / ${n(pages())}` }),
      el("button", {
        class: "button secondary small",
        type: "button",
        text: "Next",
        disabled: view.page >= pages(),
        onclick: () => {
          view.page += 1;
          changed();
        },
      }),
    );
  }

  function paint() {
    // A remembered page can be past the end once rows have gone.
    view.page = Math.min(view.page, pages());
    paintHead();
    paintBody();
    paintFoot();
  }

  paint();

  return {
    node,
    /** Replace the rows. `count` is the whole-store total in server mode. */
    update(next, count) {
      rows = next || [];
      total = count === undefined ? rows.length : count;
      if (view.page > pages()) view.page = pages();
      paint();
    },
    /** What the server should sort and page by. */
    query: () => ({ ...view }),
  };
}

/* The server-side folder picker.
 *
 * One component, used by the Index page to choose what to index and by the
 * Stores page to choose a directory to adopt. It browses the daemon's
 * filesystem rather than the browser's, because the daemon is what has to open
 * the path — a `<input type=file webkitdirectory>` hands over a list of file
 * objects and not the one thing needed, which is the path itself.
 */
function folderPicker(options) {
  const { onChoose, choose, onError, multiple } = options || {};
  const card = el("div", { class: "card picker", hidden: true });
  /* Paths ticked across however many directories were walked into. Kept out
   * here rather than per render, so browsing into a folder and back does not
   * throw away what was already chosen. */
  const ticked = new Set();

  function done() {
    card.hidden = true;
    if (onChoose) onChoose(multiple ? [...ticked] : undefined);
    ticked.clear();
  }

  async function open(path) {
    let data;
    try {
      data = await api(`/api/dirs?path=${encodeURIComponent(path || "")}`);
    } catch (e) {
      if (onError) onError(e.message);
      return false;
    }
    card.hidden = false;
    fill(
      card,
      el(
        "div",
        { class: "crumbs" },
        el(
          "button",
          {
            class: "button secondary small",
            type: "button",
            disabled: !data.parent,
            onclick: () => open(data.parent),
          },
          icon(ICONS.up, 15),
          "Up",
        ),
        el("span", { class: "where", text: data.path }),
        el("span", { class: "spacer" }),
        multiple
          ? el("button", {
              class: "button small",
              type: "button",
              text: ticked.size
                ? `Use ${ticked.size} folder${ticked.size === 1 ? "" : "s"}`
                : "Use this folder",
              onclick: () => {
                // Nothing ticked means the folder being looked at, which is
                // what pressing the button while standing in one plainly means.
                if (!ticked.size) ticked.add(data.path);
                done();
              },
            })
          : el("button", {
              class: "button small",
              type: "button",
              text: choose || "Use this folder",
              onclick: () => {
                card.hidden = true;
                if (onChoose) onChoose(data.path);
              },
            }),
        el("button", {
          class: "button secondary small",
          type: "button",
          text: "Close",
          onclick: () => {
            card.hidden = true;
          },
        }),
      ),
      el(
        "div",
        { class: "entries" },
        data.entries.length
          ? data.entries.map((entry) => {
              // A file in a folder picker is not selectable, and rendering
              // it at full contrast beside the folders that are says
              // otherwise. Dimmed when it cannot be chosen, which in the
              // multiple case is always.
              const inert = multiple && !entry.dir;
              const row = el(
                "button",
                {
                  class: inert ? "entry inert" : "entry",
                  type: "button",
                  disabled: inert,
                  title: entry.path,
                  onclick: () => {
                    if (entry.dir) open(entry.path);
                    else {
                      card.hidden = true;
                      if (onChoose) onChoose(multiple ? [entry.path] : entry.path);
                    }
                  },
                },
                icon(entry.dir ? ICONS.folder : ICONS.file),
                el("span", { class: "name", text: entry.name }),
                // What "Adopt existing .semlith" is looking for. Nothing in
                // the listing used to tell an adoptable folder from any other,
                // which is the surface of the adopt feature not working.
                entry.adoptable ? pill("a store", "good") : null,
              );
              if (!multiple || !entry.dir) return row;
              /* The tick is its own control beside the row, not the row
               * itself: walking into a folder and choosing it are different
               * intentions and one button cannot mean both. */
              const box = el("input", {
                type: "checkbox",
                class: "tick",
                "aria-label": `Index ${entry.name}`,
                checked: ticked.has(entry.path),
                onchange: () => {
                  if (box.checked) ticked.add(entry.path);
                  else ticked.delete(entry.path);
                  open(data.path);
                },
              });
              return el("div", { class: "entry-row" }, box, row);
            })
          : el("div", { class: "card pad" }, empty("Nothing here that Semlith can index.")),
      ),
    );
    return true;
  }

  return {
    node: card,
    open,
    close: () => {
      card.hidden = true;
    },
    isOpen: () => !card.hidden,
  };
}

/** A path on one line, truncated at the start, whole on hover.
 *
 * The end is the part that identifies a file: every row on the Files page
 * begins with the same `/Users/...` and differs in its last segment, so cutting
 * the end throws away the only part worth reading. */
function pathCell(value, className) {
  return el(
    "span",
    {
      class: className ? `one-line tail ${className}` : "one-line tail",
      "data-tip": value,
      // The browser's own tooltip as well as the portal's. The styled one is
      // better and it is not the only reader: a value with no `title` is a
      // truncated path nothing but the DOM inspector can recover.
      title: value,
    },
    el("bdi", { text: value }),
  );
}

/** A value on one line, truncated at the end, whole on hover. */
function lineCell(value, className) {
  return el("span", {
    class: className ? `one-line ${className}` : "one-line",
    "data-tip": value,
    title: value,
    text: value,
  });
}

/** An input with a real label. A placeholder is not one: it leaves on typing. */
function labelled(id, text, input) {
  input.id = id;
  return [el("label", { class: "sr-only", for: id, text }), input];
}

/* One page header for the whole portal.
 *
 * The title, its status pill immediately beside it, and any page actions at
 * the end of that row; the subtitle on a row of its own at the page's full
 * width. Every page uses it, so a pill is never floated to the far side of a
 * header away from the title it describes, and a subtitle is never squeezed
 * into a narrow column by a button sitting opposite it. */
function pageHead(title, subtitle, extra) {
  const { pill, actions } = extra || {};
  return el(
    "div",
    { class: "page-head" },
    el(
      "div",
      { class: "line" },
      el("h1", { text: title }),
      pill || null,
      actions ? el("span", { class: "spacer" }) : null,
      actions ? el("div", { class: "actions" }, actions) : null,
    ),
    subtitle ? el("p", { class: "subtitle", text: subtitle }) : null,
  );
}

/* A row's actions, behind one control.
 *
 * A column of buttons per row is a column of noise, and the destructive one
 * sits a mis-aimed click away from the ordinary one. The menu is a single
 * host appended to the body for the same reason the tooltip is: a popup
 * inside a cell is clipped by the table's own `overflow: auto`. */
const menu = {
  node: null,
  owner: null,

  ensure() {
    if (this.node) return this.node;
    this.node = el("div", { class: "menu", role: "menu", hidden: true });
    document.body.append(this.node);
    // One listener each, not one per menu: the host outlives every row that
    // opens it.
    document.addEventListener("pointerdown", (e) => {
      if (this.node.hidden) return;
      if (this.node.contains(e.target) || (this.owner && this.owner.contains(e.target))) return;
      this.close();
    });
    document.addEventListener("keydown", (e) => {
      if (e.key === "Escape") this.close();
    });
    window.addEventListener("resize", () => this.close());
    // A menu anchored to a row in a scrolling table has to go when the row
    // moves, rather than hang over the wrong one.
    window.addEventListener("scroll", () => this.close(), true);
    return this.node;
  },

  /** `items` are `{ label, tone, onclick }`; `at` is the control it hangs off. */
  open(at, items) {
    const node = this.ensure();
    if (this.owner === at && !node.hidden) return this.close();
    this.owner = at;
    at.setAttribute("aria-expanded", "true");
    fill(
      node,
      items.map((item) =>
        el("button", {
          class: item.tone ? `menu-item ${item.tone}` : "menu-item",
          type: "button",
          role: "menuitem",
          text: item.label,
          onclick: () => {
            this.close();
            item.onclick();
          },
        }),
      ),
    );
    node.hidden = false;
    const box = node.getBoundingClientRect();
    const rect = at.getBoundingClientRect();
    const margin = 8;
    let left = rect.right - box.width;
    left = Math.min(Math.max(margin, left), window.innerWidth - margin - box.width);
    let top = rect.bottom + 6;
    if (top + box.height > window.innerHeight - margin) top = rect.top - box.height - 6;
    node.style.left = `${Math.round(left)}px`;
    node.style.top = `${Math.round(Math.max(margin, top))}px`;
    node.querySelector(".menu-item")?.focus();
  },

  close() {
    if (!this.node) return;
    this.node.hidden = true;
    if (this.owner) this.owner.setAttribute("aria-expanded", "false");
    this.owner = null;
  },
};

/** The control that opens one, for the end of a table row. */
function rowMenu(items) {
  const button = el("button", {
    class: "button secondary small icon",
    type: "button",
    "aria-haspopup": "menu",
    "aria-expanded": "false",
    "aria-label": "Actions",
    title: "Actions",
    onclick: () => menu.open(button, items()),
  });
  fill(button, icon(ICONS.more, 18, true));
  return button;
}

/** A status pill. One shape, and a dot only where it reports a state. */
function pill(text, tone, props) {
  return el(
    "span",
    { class: tone ? `pill ${tone}` : "pill", ...(props || {}) },
    tone ? el("i", {}) : null,
    text,
  );
}

// ---------------------------------------------------------------- state

const NARROW = "(max-width: 899px)";

const state = {
  stores: [],
  theme: "light",
  navOpen: !window.matchMedia(NARROW).matches,
  /** Carried from the welcome screen into the Index view's path field. */
  pendingPath: "",
  /** Carried from a search hit into the Graph page, or into Search. */
  pendingSymbol: "",
  /** The symbol the Impact page is about, so the Graph page's "Blast radius"
   * link has somewhere to put it and coming back does not clear the answer. */
  impactSymbol: "",
  /** The store that symbol was picked in, so Impact answers about the same
   * store the Graph was showing rather than every open one. */
  impactStore: "",
  pendingQuery: "",
  /** The last search, kept so leaving the page and coming back does not throw
   * the question away along with its answers. */
  search: { query: "", stores: [] },
  /** How many clients are talking to the daemon, for the sidebar card. */
  agents: null,
  /** Whether this daemon records retrievals, for the same card. */
  ledger: null,
  /** The last `/api/index/runs` answer, shared by the navigation's count and
   * the Index page's cards. */
  runs: null,
  /** The Index page's painter while that page is on screen, so one fetch of
   * the runs route serves both readers. */
  onRuns: null,
  /** The open page's reader of a changed store list, if it has one. */
  onStores: null,
};

/* Pages, in the design's grouping. The ones the roadmap puts in a later
 * release — Inside the index, Reports, License — are deliberately absent
 * rather than stubbed: a nav item that leads nowhere is worse than one that
 * does not exist yet. Everything listed here is free on every tier. */
/* The v4 sidebar: two groups, thirteen entries, in this order.
 *
 * Doctor is the thirteenth and is not in the design's list. It is a surface
 * this binary already ships, and dropping it would leave `semlith doctor` the
 * one command with no page — the parity rule cuts both ways. */
/* The sidebar, in four groups.
 *
 * The v3 design grouped the pages as Workspace, Explore, Operate and Account,
 * and v4 flattened that to two: six pages under Workspace and seven under
 * Operate. Two groups of six and seven is a list with two headings in it —
 * long enough to scan rather than read — so this takes v3's shape back.
 * `Account` held License and About; the binary is free and there is no
 * licence page, so the last group is the two pages that describe the machine
 * this is running on.
 *
 * Every page is in exactly one group. The v4 design draws thirteen and this
 * is fourteen: `Index` and `Inside the index` are two pages there and were
 * one here, which is why the indexing controls and the corpus figures were
 * stacked on top of each other.
 */
const VIEWS = [
  { group: "Workspace", id: "stores", label: "Stores", title: "Stores" },
  { group: "Workspace", id: "files", label: "Files", title: "Files" },
  { group: "Workspace", id: "index", label: "Index", title: "Index" },
  { group: "Workspace", id: "corpus", label: "Inside the index", title: "Inside the index" },
  { group: "Explore", id: "search", label: "Search", title: "Search" },
  { group: "Explore", id: "graph", label: "Graph", title: "Graph" },
  { group: "Explore", id: "impact", label: "Impact", title: "Impact" },
  { group: "Operate", id: "ledger", label: "Retrieval ledger", title: "Retrieval ledger" },
  { group: "Operate", id: "reports", label: "Reports", title: "Reports" },
  { group: "Operate", id: "agents", label: "Agents", title: "Agents" },
  { group: "Operate", id: "cloud", label: "Cloud", title: "Cloud" },
  { group: "Operate", id: "privacy", label: "Privacy", title: "Privacy" },
  { group: "Machine", id: "doctor", label: "Doctor", title: "Doctor" },
  { group: "Machine", id: "about", label: "About", title: "About" },
];

/** The sidebar's count, kept with the data it describes rather than with the
 * render that happened to be running when it changed. */
function paintStoreCount() {
  const node = document.getElementById("daemon-stores");
  if (!node) return;
  const many = state.stores.length;
  const parts = [`${many} store${many === 1 ? "" : "s"}`];
  // Only once the answer is known: "0 agents" while the route is still in
  // flight reads as a fact rather than as a question nobody has asked yet.
  if (state.agents !== null) {
    parts.push(`${state.agents} agent${state.agents === 1 ? "" : "s"}`);
  }
  // The design's daemon card states it here, on every page: the ledger is on
  // by default and a reader should not have to open the Ledger page to find
  // out whether this daemon is recording.
  if (state.ledger !== null) parts.push(state.ledger ? "ledger on" : "ledger off");
  node.textContent = parts.join(" · ");
}

/** How many clients are connected, from whichever page last asked. */
function noteAgents(data) {
  state.agents = (data.connections || []).length;
  paintStoreCount();
}

// ----------------------------------------------------------------- live

/* One clock for the whole page.
 *
 * The daemon keeps a counter per data domain — stores, runs, clients, ledger,
 * events, privacy — and bumps it in the one function that writes that domain.
 * This polls those six integers once a second, works out which moved, and
 * refetches only those, through the routes each view already uses. A quiet
 * daemon with a tab open therefore costs one small request a second and
 * nothing else.
 *
 * It is a poll rather than a server-sent stream because a stream holds one of
 * the daemon's eight HTTP workers for as long as the tab is open, which is the
 * constraint the whole run-state design exists to respect. Six integers a
 * second is cheaper than the connection would be and cannot exhaust the pool.
 *
 * Every live view registers here. No view starts a timer of its own: a second
 * timer is a second clock, and two clocks are how one panel updates and the
 * one beside it does not. */
const live = {
  /** The counter value each domain was last acted on at. */
  seen: {},
  /** `{ domains, run }`, cleared on every navigation. */
  watchers: [],
  timer: null,
};

/** Ask to be called when any of these domains is written. */
function watchLive(domains, run) {
  live.watchers.push({ domains, run });
}

/** Drop every watcher. Called as a view is replaced, so nothing left behind
 * keeps refetching for a page that is no longer on screen. */
function resetLive() {
  live.watchers = [];
}

async function pollChanges() {
  // A hidden tab asks nothing. Its `seen` values stay where they were, so the
  // first poll after it comes back sees everything that moved meanwhile and
  // fires each watcher once rather than once per missed second.
  if (document.visibilityState === "hidden") return;
  let counters;
  try {
    counters = await api("/api/changes");
  } catch (_) {
    // A daemon that has stopped answering is not a reason to tear the page
    // down; the next tick tries again.
    return;
  }
  const moved = [];
  for (const [domain, value] of Object.entries(counters)) {
    // The first read is a baseline, not a change: everything the page drew on
    // load is already current.
    if (live.seen[domain] === undefined) {
      live.seen[domain] = value;
      continue;
    }
    if (live.seen[domain] !== value) {
      live.seen[domain] = value;
      moved.push(domain);
    }
  }
  if (!moved.length) return;
  /* The sidebar's store count is on every page, so it follows the stores
   * domain on every page rather than only where a view asked. A store deleted
   * by a stop on the Index page used to stay counted until a reload. */
  if (moved.includes("stores")) {
    refreshStores()
      .then(() => state.onStores && state.onStores())
      .catch(() => {});
  }
  for (const watcher of live.watchers) {
    if (watcher.domains.some((domain) => moved.includes(domain))) {
      try {
        watcher.run(moved);
      } catch (_) {
        /* one view's refresh failing must not stop the others' */
      }
    }
  }
}

function startLive() {
  if (live.timer) return;
  live.timer = setInterval(pollChanges, 1000);
  // Straight away on return rather than up to a second later: coming back to a
  // tab and watching it sit stale is the thing this replaces.
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") pollChanges();
  });
}

/** Every run the daemon knows about, refetched when the runs domain moves.
 *
 * One fetch serves both readers — the navigation's count on every page, and
 * the Index page's cards — because two fetches of one route is how a count in
 * the sidebar disagrees with the cards beside it. */
let runsAsked = 0;
let runsPainted = 0;

/** Make every runs read still in flight too old to paint. */
function supersedeRuns() {
  runsPainted = ++runsAsked;
}

async function refreshRuns() {
  // Numbered, because two can be in flight: the poll's, and the one a button
  // makes after its own POST. The poll's can be the older question and still
  // arrive second, and painting it put "Pause" back on a button that had just
  // turned to "Resume" for a second before the next poll turned it back.
  const mine = ++runsAsked;
  let data;
  try {
    data = await api("/api/index/runs");
  } catch (_) {
    return state.runs;
  }
  if (mine < runsPainted) return state.runs;
  runsPainted = mine;
  state.runs = data;
  paintRunCount();
  if (state.onRuns) state.onRuns(data);
  return data;
}

/** "2 indexing" in the navigation, on every page, so a run is never something
 * that happened out of sight. */
function paintRunCount() {
  const on = (state.runs?.runs || []).filter((run) => TICKING.has(run.status) && run.status !== "review").length;
  const waiting = (state.runs?.queue || []).length;
  // A run waiting for a person is not indexing; it is named apart.
  const review = (state.runs?.runs || []).filter((run) => run.status === "review").length;
  for (const node of document.querySelectorAll(".run-count")) {
    const parts = [];
    if (on) parts.push(`${on} indexing`);
    if (waiting) parts.push(`${waiting} queued`);
    if (review) parts.push(`${review} waiting for review`);
    node.textContent = parts.join(" · ");
    node.hidden = !parts.length;
  }
}

/** The statuses a run is still in. */
const TICKING = new Set(["queued", "review", "running", "pausing", "paused", "held", "stopping"]);

/** Draw the view on screen again, keeping where the reader had scrolled to.
 *
 * What a live table wants is the rows it would have had on a reload, and the
 * view functions already know how to produce exactly that from the routes they
 * read. Redrawing them is therefore one line per page rather than a second,
 * incremental renderer per page — and a second renderer is how a table ends up
 * disagreeing with the reload of itself.
 *
 * The Index page does not use this: its cards hold logs and scroll positions
 * of their own, so it paints in place. */
async function repaintView() {
  if (!shell.main) return;
  const at = scrollHolder()?.scrollTop ?? 0;
  await render();
  const holder = scrollHolder();
  if (holder) holder.scrollTop = at;
}

/* The element that actually scrolls on the page that is open.
 *
 * This used to read `.scroller`, which never scrolled: `.view` carries
 * `overflow-y: auto` and is the scroll container, and `.scroller` is a stack
 * inside it that takes the leftover height. So a live repaint of the ledger or
 * the Stores table read 0 and wrote 0 back, and a reader watching rows arrive
 * was returned to the top every time one did. `.view` first, and `.scroller`
 * kept after it for any page that grows its own inner scroller later. */
function scrollHolder() {
  if (!shell.main) return null;
  return shell.main.querySelector(".view") || shell.main.querySelector(".scroller");
}


// ---------------------------------------------------------------- graph

/* The colours the canvas draws with, read from the stylesheet rather than
 * repeated here: the page is themed by CSS custom properties, and a canvas
 * cannot inherit one. Re-read on every paint so a theme switch is picked up. */
function graphInk() {
  const style = getComputedStyle(document.documentElement);
  const read = (name, fallback) => (style.getPropertyValue(name) || fallback).trim();
  return {
    edge: read("--line", "#dce3e8"),
    hot: read("--blue", "#4c7088"),
    node: read("--panel", "#ffffff"),
    nodeLine: read("--line", "#dce3e8"),
    text: read("--ink-2", "#3a4d5e"),
    nearFill: read("--blue-soft", "#e3ecf2"),
    nearLine: read("--blue", "#4c7088"),
    nearText: read("--ink", "#1e2a35"),
    // The tone an ambiguous edge is drawn in: the same amber the badge uses,
    // so the canvas and the rail say one thing.
    warn: read("--amber-ink", "#7a4e0a"),
    sel: read("--accent", "#f0a43c"),
    selLine: read("--accent-edge", "#c97f14"),
    selText: read("--accent-ink", "#1e2a35"),
    muted: read("--muted", "#64778a"),
  };
}

/* Stores that could not be read, over a page drawn from the ones that could.
 *
 * Issue #129: one store whose database cannot be opened used to make every
 * route that reads across stores answer `500 disk I/O error`, so the Graph page
 * drew nothing and the message named neither the store nor the fact that the
 * others were fine. The routes answer partially now, and this is the other half
 * — a page that quietly returned four stores' results where a reader expected
 * five would be the same defect wearing a 200.
 *
 * Derived from the response every time, never accumulated: a store that comes
 * back — a drive plugged in again — is simply absent from the next `failed`, and
 * a notice that had remembered it would outlive the problem.
 *
 * No button. The remedy is `semlith drop`, and a one-click drop beside what may
 * be an unplugged volume is how somebody loses a store they could have had back
 * by plugging it in. The command is text they run themselves.
 */
function unreadableNotice(failed) {
  if (!failed || !failed.length) return null;
  const many = failed.length > 1;
  return el(
    "div",
    { class: "notice bad" },
    el("div", {
      class: "what",
      // The consequence first, because it is what changes how the page under
      // this is read: these results are short by a store.
      text: `${failed.length} store${many ? "s" : ""} could not be read, so ${
        many ? "they are" : "it is"
      } not in these results.`,
    }),
    el(
      "div",
      { class: "rows tight" },
      failed.map((store) =>
        el(
          "details",
          { class: "unreadable" },
          el(
            "summary",
            {},
            el("span", { class: "name", text: store.store }),
            el("span", { class: "meta one-line", "data-tip": store.path, text: store.path }),
          ),
          el("p", { class: "subtitle", text: store.remedy }),
          // The whole chain, behind the disclosure. It is the evidence, not
          // the message: `database disk image is malformed: Error code 11: …`
          // is what a reader pastes into an issue, not what tells them what
          // happened.
          el("pre", { class: "code", text: store.error }),
        ),
      ),
    ),
  );
}

/** The last two segments of a path: enough to recognise, short enough to read. */
function shortPath(path) {
  const parts = String(path).split("/").filter(Boolean);
  return parts.slice(-2).join("/") || path;
}

/** True when the viewer has asked for less movement. */
function stillness() {
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/* A force-directed canvas of labelled boxes.
 *
 * Positions are kept in 0..1 and multiplied by the canvas size on every frame,
 * so a resize moves the graph rather than rebuilding it, and the constants
 * below mean the same thing at any window width. They are the design bundle's:
 * repulsion 5200 over distance squared, a spring at rest 138px, damping 0.86,
 * and a weak pull to the centre.
 *
 * ponytail: O(n²) repulsion over every pair each frame. The node budget is 180,
 * so that is ~16k pairs — fine at 60fps. Past a few hundred this wants a
 * quadtree, not a faster loop.
 */
function graphCanvas(options) {
  const { onPick, onHover } = options || {};
  const canvas = el("canvas", { class: "graph-canvas" });
  const wrap = el("div", { class: "graph-stage" }, canvas);
  let nodes = [];
  let edges = [];
  /* Every symbol the scope holds, which is not the same as the number drawn:
   * a force layout is readable at dozens of nodes and a hairball at hundreds,
   * so the view is capped. Saying only what is drawn let the summary read as a
   * statement about the whole graph, and then scoping to a symbol could report
   * more edges than "the whole graph" had. */
  let total = 0;
  let drawn = [];
  let selected = null;
  let near = new Set();
  let hovered = null;
  let dragging = null;
  let down = null;
  let frame = null;
  let running = true;
  let settled = false;
  /* Whether this canvas has ever been in the document. See `tick`. */
  let attached = false;
  /* How much of the force is still applied. The layout cools, but never to
   * nothing: it bottoms out at `ALPHA_FLOOR`, so the springs keep holding the
   * shape while the drift below moves it. Anything that changes the layout —
   * a drag, a new scope, an edge-kind filter — re-heats it. */
  let alpha = 1;
  /* Springs are shared out by how many edges a node carries. A hub with forty
   * edges feels forty pulls where a leaf feels one, and at this node count that
   * is what turns the simulation into a two-frame bounce: every node overshoots
   * its rest position, and the next step overshoots back. Dividing by the
   * square root of the degree is what keeps a dense neighbourhood stable. */
  let load = [];

  function size() {
    const ratio = window.devicePixelRatio || 1;
    const box = wrap.getBoundingClientRect();
    const w = Math.max(box.width, 240);
    const h = Math.max(box.height, 240);
    canvas.width = Math.round(w * ratio);
    canvas.height = Math.round(h * ratio);
    const ctx = canvas.getContext("2d");
    ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
    return { w, h, ctx };
  }

  /* The furthest a node may move in one step, in fractions of the canvas.
   * Without it a single large force sends a node across the frame and the
   * spring drags it back, which is the bounce this cap exists to stop. */
  const MAX_STEP = 0.012;

  /* The layout never freezes. Cooling to a standstill was what stopped the
   * two-frame bounce, but a still picture is not what this view is for, so the
   * heat bottoms out here and every node carries a slow wander on top of it.
   *
   * The wander is a sine of the clock and the node's own phase, not a random
   * nudge: a random walk accumulates, drags nodes off their springs and is
   * indistinguishable from the jitter this replaced. A smooth curve at this
   * amplitude settles at under a pixel a frame, which reads as a drift. */
  const ALPHA_FLOOR = 0;
  const DRIFT = 0.00003;

  /* The box a node's centre may sit in, in fractions of the canvas.
   *
   * It used to be a flat 6% margin, which is a margin for a dot and not for a
   * label: a box 200px wide on a 570px canvas has 100px of itself outside the
   * frame at 0.94, so the Impact card sliced its outermost names in half. The
   * margin is half the node's own measured width, so every label lands inside
   * whatever canvas it is drawn on. Capped at 0.45 for the case where one
   * label is wider than the canvas, which has nowhere to be put.
   */
  function pen(node, w, h) {
    return {
      mx: Math.min(0.45, ((node.w || 80) / 2 + 4) / w),
      my: Math.min(0.45, ((node.h || 28) / 2 + 4) / h),
    };
  }

  function step(w, h) {
    const clock = performance.now() * 0.00028;
    for (let i = 0; i < nodes.length; i++) {
      const a = nodes[i];
      for (let j = i + 1; j < nodes.length; j++) {
        const b = nodes[j];
        let dx = (b.x - a.x) * w;
        let dy = (b.y - a.y) * h;
        let d2 = dx * dx + dy * dy;
        if (d2 < 1) {
          // Exactly coincident nodes have no direction to separate along.
          d2 = 1;
          dx = 0.6;
          dy = 0.4;
        }
        const d = Math.sqrt(d2);
        const force = (5200 / d2) * alpha;
        a.vx -= ((dx / d) * force) / w;
        a.vy -= ((dy / d) * force) / h;
        b.vx += ((dx / d) * force) / w;
        b.vy += ((dy / d) * force) / h;
      }
    }
    for (const edge of drawn) {
      const a = nodes[edge.from];
      const b = nodes[edge.to];
      if (!a || !b) continue;
      const dx = (b.x - a.x) * w;
      const dy = (b.y - a.y) * h;
      const d = Math.max(1, Math.sqrt(dx * dx + dy * dy));
      const pull = (d - 138) * 0.0025 * alpha;
      // Shared out by degree at each end, so a hub is not dragged about by
      // every edge it happens to carry.
      const ax = Math.sqrt(load[edge.from] || 1);
      const bx = Math.sqrt(load[edge.to] || 1);
      a.vx += ((((dx / d) * pull * d) / w) * 0.02) / ax;
      a.vy += ((((dy / d) * pull * d) / h) * 0.02) / ax;
      b.vx -= ((((dx / d) * pull * d) / w) * 0.02) / bx;
      b.vy -= ((((dy / d) * pull * d) / h) * 0.02) / bx;
    }
    let moved = 0;
    for (const node of nodes) {
      node.vx += (0.5 - node.x) * 0.006 * alpha;
      node.vy += (0.5 - node.y) * 0.008 * alpha;
      node.vx += Math.cos(clock + node.phase) * DRIFT;
      node.vy += Math.sin(clock * 0.9 + node.phase * 1.7) * DRIFT;
      if (node === dragging) {
        node.vx = 0;
        node.vy = 0;
        continue;
      }
      node.vx *= 0.86;
      node.vy *= 0.86;
      // One step can only take a node so far. A force large enough to throw it
      // across the frame is a force the spring will undo next step, which is
      // the two-frame bounce rather than a layout.
      node.vx = Math.max(-MAX_STEP, Math.min(MAX_STEP, node.vx));
      node.vy = Math.max(-MAX_STEP, Math.min(MAX_STEP, node.vy));
      moved += Math.abs(node.vx) + Math.abs(node.vy);
      // Kept inside the frame: a node that drifts off-canvas is a node nobody
      // can click, and a label half outside it is a name nobody can read.
      const bound = pen(node, w, h);
      node.x = Math.min(1 - bound.mx, Math.max(bound.mx, node.x + node.vx));
      node.y = Math.min(1 - bound.my, Math.max(bound.my, node.y + node.vy));
    }
    alpha = Math.max(ALPHA_FLOOR, alpha * 0.985);
    return moved;
  }

  /** Put the heat back in, for anything that changes the layout. */
  function reheat(to) {
    alpha = Math.max(alpha, to === undefined ? 0.35 : to);
  }

  /* Push overlapping labels apart, after the springs have had their say.
   *
   * The force layout solves for edge length and node repulsion and knows
   * nothing about how wide a label is, so in the default view
   * `the_queue_admits_in_subm…` sat on top of `a_dequeued_run_is_answere…` and
   * one label was worn down to `…as`. This works on the boxes the last paint
   * measured, which is the only place their real width exists.
   *
   * ponytail: O(n²) over every pair, four passes. The node budget is 180, so
   * that is 130k comparisons once per layout change; a grid would be the
   * upgrade if the budget ever rises. */
  function separate() {
    const box = wrap.getBoundingClientRect();
    const w = Math.max(box.width, 240);
    const h = Math.max(box.height, 240);
    for (let pass = 0; pass < 4; pass++) {
      for (let i = 0; i < nodes.length; i++) {
        for (let j = i + 1; j < nodes.length; j++) {
          const a = nodes[i];
          const b = nodes[j];
          // Before the first paint there are no measurements, and a guess
          // here would push the layout around for no reason.
          if (!a.w || !b.w) return;
          const dx = (b.x - a.x) * w;
          const dy = (b.y - a.y) * h;
          const wantX = (a.w + b.w) / 2 + 8;
          const wantY = (a.h + b.h) / 2 + 6;
          const overX = wantX - Math.abs(dx);
          const overY = wantY - Math.abs(dy);
          // Boxes only collide when they overlap on both axes.
          if (overX <= 0 || overY <= 0) continue;
          // Along whichever axis needs the smaller move, so a label is nudged
          // aside rather than thrown across the canvas.
          const pa = pen(a, w, h);
          const pb = pen(b, w, h);
          if (overX / w < overY / h) {
            const push = ((dx >= 0 ? 1 : -1) * overX) / w / 2;
            a.x = Math.min(1 - pa.mx, Math.max(pa.mx, a.x - push));
            b.x = Math.min(1 - pb.mx, Math.max(pb.mx, b.x + push));
          } else {
            const push = ((dy >= 0 ? 1 : -1) * overY) / h / 2;
            a.y = Math.min(1 - pa.my, Math.max(pa.my, a.y - push));
            b.y = Math.min(1 - pb.my, Math.max(pb.my, b.y + push));
          }
        }
      }
    }
  }

  /** A rounded rectangle, the shape every symbol is drawn as. */
  function box(ctx, x, y, w, h, r) {
    ctx.beginPath();
    ctx.moveTo(x + r, y);
    ctx.arcTo(x + w, y, x + w, y + h, r);
    ctx.arcTo(x + w, y + h, x, y + h, r);
    ctx.arcTo(x, y + h, x, y, r);
    ctx.arcTo(x, y, x + w, y, r);
    ctx.closePath();
  }

  function paint() {
    const { w, h, ctx } = size();
    const ink = graphInk();
    ctx.clearRect(0, 0, w, h);

    // Measured first: an edge stops at the box it points into, which means
    // every box's size has to be known before the first edge is drawn.
    ctx.font = '500 11px "IBM Plex Mono", ui-monospace, monospace';
    for (const node of nodes) {
      const ease = stillness() ? (node === hovered ? 1 : 0) : node.hover || 0;
      node.hover = ease;
      const text = ctx.measureText(node.label).width;
      node.w = (text + 22) * (1 + 0.05 * ease);
      node.h = 28 * (1 + 0.05 * ease);
    }

    for (const edge of drawn) {
      const a = nodes[edge.from];
      const b = nodes[edge.to];
      if (!a || !b) continue;
      const ax = a.x * w;
      const ay = a.y * h;
      const bx = b.x * w;
      const by = b.y * h;
      const hot = edge.from === selected || edge.to === selected;
      const angle = Math.atan2(by - ay, bx - ax);
      // To the edge of the box rather than to its centre, so the arrowhead
      // lands where the reader sees the symbol begin.
      const inset = Math.min(b.w / 2 + 6, Math.abs(b.h / 2 / Math.sin(angle) || b.w));
      const ex = bx - Math.cos(angle) * Math.min(inset, b.w / 2 + 6);
      const ey = by - Math.sin(angle) * (b.h / 2 + 3);
      ctx.save();
      ctx.strokeStyle = hot ? ink.hot : ink.edge;
      ctx.fillStyle = hot ? ink.hot : ink.edge;
      ctx.lineWidth = hot ? 1.7 : 1.1;
      // Drawn with the same four values the rail shows. An edge the source
      // settled is solid; one matched by bare name is dotted; one that could
      // mean several different definitions is dashed and drawn in the warning
      // tone, because it is not a claim about this code at all.
      const settled = edge.confidence === "extracted" || edge.confidence === "resolved";
      if (edge.confidence === "ambiguous") {
        ctx.strokeStyle = hot ? ink.hot : ink.warn;
        ctx.fillStyle = hot ? ink.hot : ink.warn;
        ctx.setLineDash([6, 4]);
      } else {
        ctx.setLineDash(settled ? [] : [2, 3]);
      }
      ctx.beginPath();
      ctx.moveTo(ax, ay);
      ctx.lineTo(ex, ey);
      ctx.stroke();
      ctx.setLineDash([]);
      ctx.beginPath();
      ctx.moveTo(ex, ey);
      ctx.lineTo(ex - Math.cos(angle - 0.4) * 7, ey - Math.sin(angle - 0.4) * 7);
      ctx.lineTo(ex - Math.cos(angle + 0.4) * 7, ey - Math.sin(angle + 0.4) * 7);
      ctx.closePath();
      ctx.fill();
      ctx.restore();
    }

    nodes.forEach((node, i) => {
      const on = i === selected;
      const beside = near.has(i);
      const x = node.x * w - node.w / 2;
      const y = node.y * h - node.h / 2;
      box(ctx, x, y, node.w, node.h, 8);
      ctx.fillStyle = on ? ink.sel : beside ? ink.nearFill : ink.node;
      ctx.save();
      if (node.hover) {
        ctx.shadowColor = ink.nearLine;
        ctx.shadowBlur = 14 * node.hover;
      }
      ctx.fill();
      ctx.restore();
      ctx.lineWidth = (on ? 1.6 : 1) + 0.8 * node.hover;
      ctx.strokeStyle = on ? ink.selLine : beside ? ink.nearLine : ink.nodeLine;
      ctx.stroke();
      ctx.fillStyle = on ? ink.selText : beside ? ink.nearText : ink.text;
      ctx.fillText(node.label, x + 11, y + node.h / 2 + 4);
    });
  }

  function tick() {
    // A view that has been navigated away from is not worth animating. The
    // canvas is built before it is inserted, so the first frames legitimately
    // run detached — only a canvas that *was* in the page and is not any more
    // has been thrown away.
    if (wrap.isConnected) attached = true;
    else if (attached) {
      frame = null;
      running = false;
      observer.disconnect();
      return;
    }
    const box = wrap.getBoundingClientRect();
    if (running || dragging) step(Math.max(box.width, 240), Math.max(box.height, 240));
    // Hover is eased rather than switched, so a fast pointer sweep across a
    // crowded graph does not strobe.
    for (const node of nodes) {
      node.hover = (node.hover || 0) + ((node === hovered ? 1 : 0) - (node.hover || 0)) * 0.16;
      if (node.hover < 0.002) node.hover = 0;
    }
    paint();
    frame = requestAnimationFrame(tick);
  }

  function at(event) {
    const box = canvas.getBoundingClientRect();
    return { x: event.clientX - box.left, y: event.clientY - box.top, w: box.width, h: box.height };
  }

  function hit(point) {
    for (let i = nodes.length - 1; i >= 0; i--) {
      const node = nodes[i];
      const cx = node.x * point.w;
      const cy = node.y * point.h;
      if (
        Math.abs(point.x - cx) < (node.w || 80) / 2 &&
        Math.abs(point.y - cy) < (node.h || 28) / 2
      ) {
        return i;
      }
    }
    return null;
  }

  function selectAt(index) {
    selected = index;
    near = new Set();
    for (const edge of drawn) {
      if (edge.from === index) near.add(edge.to);
      if (edge.to === index) near.add(edge.from);
    }
  }

  canvas.addEventListener("pointerdown", (event) => {
    const index = hit(at(event));
    if (index === null) return;
    canvas.setPointerCapture(event.pointerId);
    dragging = nodes[index];
    reheat();
    down = { x: event.clientX, y: event.clientY, index };
    canvas.classList.add("dragging");
    event.preventDefault();
  });

  canvas.addEventListener("pointermove", (event) => {
    const point = at(event);
    if (dragging) {
      const bound = pen(dragging, point.w, point.h);
      dragging.x = Math.min(1 - bound.mx, Math.max(bound.mx, point.x / point.w));
      dragging.y = Math.min(1 - bound.my, Math.max(bound.my, point.y / point.h));
      if (!frame) paint();
      return;
    }
    const index = hit(point);
    const node = index === null ? null : nodes[index];
    canvas.classList.toggle("over-node", Boolean(node));
    if (node !== hovered) {
      hovered = node;
      if (!frame) paint();
    }
    if (onHover) onHover(node, event.clientX, event.clientY);
  });

  const release = (event) => {
    if (down) {
      // A drag that went nowhere is a click. Four pixels is the slack a
      // pointer has on the way down.
      const moved = Math.abs(event.clientX - down.x) + Math.abs(event.clientY - down.y);
      if (moved < 4) {
        selectAt(down.index);
        if (onPick) onPick(nodes[down.index]);
      }
    }
    dragging = null;
    down = null;
    canvas.classList.remove("dragging");
    if (!frame) paint();
  };
  canvas.addEventListener("pointerup", release);
  canvas.addEventListener("pointercancel", release);
  canvas.addEventListener("pointerleave", () => {
    hovered = null;
    dragging = null;
    down = null;
    canvas.classList.remove("dragging", "over-node");
    if (onHover) onHover(null);
    if (!frame) paint();
  });

  // A canvas has no layout of its own to react to a resize with, and the
  // stylesheet cannot repaint it.
  const observer = new ResizeObserver(() => {
    if (!frame) paint();
  });
  observer.observe(wrap);

  return {
    node: wrap,
    draw(data, kinds) {
      nodes = (data.nodes || []).map((row, i) => {
        const angle = (i / Math.max((data.nodes || []).length, 1)) * Math.PI * 2;
        return {
          ...row,
          // Truncated for the canvas only. Test names run past sixty
          // characters, and a box that wide covers its neighbours and pushes
          // the layout off the frame; the whole name is in the hover card and
          // in the rail the moment the node is picked.
          label: row.name.length > 26 ? `${row.name.slice(0, 25)}…` : row.name,
          x: 0.5 + Math.cos(angle) * 0.28,
          y: 0.5 + Math.sin(angle) * 0.28,
          vx: 0,
          vy: 0,
          // Where this node is in its own wander, so forty nodes drift apart
          // rather than sliding as one block.
          phase: Math.random() * Math.PI * 2,
          hover: 0,
          callers: 0,
          callees: 0,
        };
      });
      edges = data.edges || [];
      total = data.total || 0;
      selected = null;
      near = new Set();
      hovered = null;
      this.filter(kinds);
      // Settled before the first painted frame, rather than exploding outward
      // while the reader watches.
      alpha = 1;
      const box = wrap.getBoundingClientRect();
      for (let i = 0; i < 220; i++) {
        step(Math.max(box.width, 240), Math.max(box.height, 240));
      }
      settled = true;
      // Once for the measurements, then the labels are pushed off each other
      // using them, then again to draw the result.
      paint();
      separate();
      paint();
    },
    /** Draw only the edge kinds asked for. Returns how many are drawn.
     *
     * An empty set means no kind is selected, which means no edges — not
     * every edge. The old "no filter selected means no filter" fallback was
     * indistinguishable from all six chips on, and the summary line asserted
     * a number that did not describe what was drawn. */
    filter(kinds) {
      drawn = kinds ? edges.filter((e) => kinds.has(e.kind)) : edges;
      // Counted over what is drawn rather than over what was fetched: a hover
      // card that says "3 in" beside one line on the canvas is describing a
      // graph the reader cannot see, and the reader believes the card.
      load = nodes.map(() => 1);
      for (const node of nodes) {
        node.callers = 0;
        node.callees = 0;
      }
      for (const edge of drawn) {
        if (nodes[edge.from]) nodes[edge.from].callees += 1;
        if (nodes[edge.to]) nodes[edge.to].callers += 1;
        if (load[edge.from] !== undefined) load[edge.from] += 1;
        if (load[edge.to] !== undefined) load[edge.to] += 1;
      }
      if (selected !== null) selectAt(selected);
      // Fewer edges is a different layout, so the springs get another go at it.
      reheat(0.25);
      if (settled) paint();
      return drawn.length;
    },
    /** Select a node by name, as though it had been clicked. */
    pick(name) {
      const index = nodes.findIndex((node) => node.name === name);
      if (index < 0) return false;
      selectAt(index);
      paint();
      if (onPick) onPick(nodes[index]);
      return true;
    },
    counts: () => ({ nodes: nodes.length, edges: drawn.length, total }),

    /** The symbol worth landing on: the one with the most edges that were
     * actually found rather than guessed.
     *
     * The unscoped view used to open on whatever had the most edges of any
     * kind, which in a Rust codebase is `new` — every type has one, and its
     * neighbourhood is hundreds of inferred edges to unrelated code. A hub of
     * extracted and resolved edges is a hub of the corpus rather than a hub of
     * one very common word. */
    best() {
      const score = new Map();
      for (const edge of drawn) {
        if (edge.confidence !== "extracted" && edge.confidence !== "resolved") continue;
        for (const end of [edge.from, edge.to]) {
          score.set(end, (score.get(end) || 0) + 1);
        }
      }
      let pick = null;
      let most = 0;
      for (const [index, count] of score) {
        if (count <= most || !nodes[index]) continue;
        most = count;
        pick = nodes[index].name;
      }
      return pick;
    },

    /** Spread the layout back out to fill the frame. */
    fit() {
      if (!nodes.length) return;
      const xs = nodes.map((node) => node.x);
      const ys = nodes.map((node) => node.y);
      const spread = (values, low, high) => {
        const min = Math.min(...values);
        const max = Math.max(...values);
        const span = max - min;
        // Everything in one spot: nothing to spread, and dividing by the span
        // would be dividing by zero.
        if (span < 0.001) return () => (low + high) / 2;
        return (value) => low + ((value - min) / span) * (high - low);
      };
      const toX = spread(xs, 0.08, 0.92);
      const toY = spread(ys, 0.1, 0.9);
      for (const node of nodes) {
        node.x = toX(node.x);
        node.y = toY(node.y);
        node.vx = 0;
        node.vy = 0;
      }
      separate();
      paint();
    },
    running: () => running,
    toggle() {
      running = !running;
      if (running) reheat(0.2);
      return running;
    },
    start() {
      // Under reduced motion the graph is solved once and then still: there is
      // no loop to pause, and nothing moves unless the reader moves it.
      if (stillness()) {
        running = false;
        paint();
        return;
      }
      if (!frame) tick();
    },
    stop() {
      running = false;
      if (frame) cancelAnimationFrame(frame);
      frame = null;
      observer.disconnect();
    },
    repaint: paint,
  };
}

// Every kind `graph::KINDS` stores. A kind missing here is fetched from
// /api/graph and thrown away before painting, which is what `contains` and
// `aliases` were until 0.17.2 - the rail counted them and the canvas did not
// draw them. `tests/portal.rs` asserts this list against the Rust one.
const EDGE_KINDS = ["defines", "calls", "imports", "references", "contains", "aliases"];

async function graphView() {
  await refreshStores();

  const kinds = new Set(EDGE_KINDS);
  const chosen = new Set();
  const meta = el("span", { class: "graph-count" });
  const rail = el("div", { class: "graph-rail" });
  /* The selected symbol's two actions, in a footer of the side column that
   * does not scroll with it. Inside the rail they were sticky against a
   * padding the rail stopped having when the column became the scroller, so
   * they sat pinned just below the visible edge, cut in half. */
  const actions = el("div", { class: "rail-actions", hidden: true });
  const unreadable = el("div", { class: "unreadable-slot" });

  const canvas = graphCanvas({
    onPick: (node) => select(node),
    onHover: (node, x, y) => {
      if (!node) return tip.hide("graph");
      tip.atPoint(
        x,
        y,
        tipCard(node.name, "blue", [
          ["kind", node.kind],
          ["file", `${shortPath(node.path)}:${node.start_line}-${node.end_line}`],
          ["store", node.store || (state.stores[0] && state.stores[0].name) || "—"],
          ["calls", `${node.callers} in · ${node.callees} out`],
        ]),
        "graph",
      );
    },
  });

  const pause = el("button", {
    class: "button secondary small",
    type: "button",
    text: "Pause",
    onclick: (e) => {
      const on = canvas.toggle();
      e.currentTarget.textContent = on ? "Pause" : "Resume";
    },
  });

  /* Beside Pause, which used to be the only control the canvas had: a reader
   * who dragged a node off the edge had no way back short of reloading. */
  const fit = el("button", {
    class: "button secondary small",
    type: "button",
    text: "Fit",
    onclick: () => canvas.fit(),
  });
  const reset = el("button", {
    class: "button secondary small",
    type: "button",
    text: "Reset",
    onclick: () => load({}),
  });

  /* The label has to come from what the canvas is actually doing, not from a
   * second reading of the media query: under reduced motion `start` never
   * begins a loop, and a button reading "Pause" beside a still picture is a
   * lie about which of the two is in charge. */
  function paintPause() {
    pause.textContent = canvas.running() ? "Pause" : "Settled";
  }

  function counts() {
    const { nodes, edges, total } = canvas.counts();
    // What is drawn, out of what the scope holds. The cap is the reason the
    // two differ, and a summary that mentioned only the first read as a count
    // of the whole graph.
    const shown = total && total > nodes ? `${n(nodes)} of ${n(total)} symbols` : `${n(nodes)} symbols`;
    meta.textContent = `${shown} · ${n(edges)} edges`;
  }

  function blank(message) {
    actions.hidden = true;
    return fill(
      rail,
      el("div", { class: "graph-selected" }, el("div", { class: "rail-hint", text: message })),
    );
  }

  function ends(title, list, absent, extra) {
    return el(
      "div",
      { class: "rail-group" },
      el("h3", { text: title }),
      list.length
        ? el(
            "ul",
            { class: "rail-list" },
            list.map((end) => endRow(end)),
          )
        : el("div", { class: "rail-hint", text: absent }),
      extra || null,
    );
  }

  /* One neighbour. An ambiguous row stands for several definitions and must
   * not name one of them: a reader who follows it would open one of four files
   * the call could have meant. It says how many instead, and opens on ask. */
  function endRow(end) {
    if (end.confidence === "ambiguous") {
      const li = el("li", { class: "ambiguous-row" });
      let open = false;
      const toggle = el("button", {
        class: "link-button",
        type: "button",
        text: `${end.name} · ${end.definitions} definitions`,
        "aria-expanded": "false",
        onclick: async () => {
          open = !open;
          toggle.setAttribute("aria-expanded", String(open));
          if (!open) return fill(nested);
          fill(nested, skeletonRows(2));
          let all;
          try {
            all = await api(
              scoped(`/api/neighbors?name=${encodeURIComponent(state.graphSelected)}&all=1`),
            );
          } catch (e) {
            return fill(nested, error(e.message));
          }
          const each = all.callees.filter((c) => c.name === end.name);
          fill(
            nested,
            el(
              "ul",
              { class: "rail-list nested" },
              each.map((one) =>
                el(
                  "li",
                  {},
                  el("a", {
                    href: `#graph?name=${encodeURIComponent(one.name)}`,
                    text: `${shortPath(one.path)}:${one.start_line}`,
                    onclick: (e) => {
                      e.preventDefault();
                      focus(one.name);
                    },
                  }),
                ),
              ),
            ),
          );
        },
      });
      const nested = el("div", { class: "nested-wrap" });
      return fill(li, toggle, confidenceBadge(end.confidence), el("span", { class: "via", text: end.kind }), nested);
    }
    return el(
      "li",
      {},
      el("a", {
        href: `#graph?name=${encodeURIComponent(end.name)}`,
        text: end.name,
        title: end.name,
        onclick: (e) => {
          e.preventDefault();
          focus(end.name);
        },
      }),
      confidenceBadge(end.confidence),
      el("span", { class: "via", text: end.kind }),
    );
  }

  /* What the four values mean, once, under the rail. A badge whose meaning a
   * reader has to guess is a badge that gets read as decoration. */
  function confidenceLegend() {
    return el(
      "div",
      { class: "conf-legend" },
      Object.entries(CONFIDENCE).map(([value, [label, why]]) =>
        el(
          "span",
          { class: "legend-chip" },
          el("span", { class: `conf ${value}`, text: label }),
          el("span", { class: "why", text: why }),
        ),
      ),
    );
  }

  async function select(node) {
    actions.hidden = true;
    fill(rail, skeletonRows(6));
    let data;
    try {
      data = await api(scoped(`/api/neighbors?name=${encodeURIComponent(node.name)}`));
    } catch (e) {
      return fill(rail, error(e.message));
    }
    state.graphSelected = node.name;
    const edge = (e) => ({
      name: e.name,
      kind: e.kind,
      confidence: e.confidence,
      definitions: e.definitions,
      path: e.path,
      start_line: e.start_line,
    });

    fill(
      rail,
      el(
        "div",
        { class: "graph-selected" },
        el("span", { class: "eyebrow", text: "Selected symbol" }),
        el("h2", { class: "sym", text: node.name, title: node.name }),
        el("div", {
          class: "loc",
          "data-tip": node.path,
          text: `${shortPath(node.path)}:${node.start_line}-${node.end_line}`,
        }),
        el(
          "div",
          { class: "chips" },
          el("span", { class: "chip static blue", text: node.kind }),
          node.store ? el("span", { class: "chip static", text: node.store }) : null,
          el("span", {
            class: "chip static",
            text: `${Math.max(node.end_line - node.start_line + 1, 1)} lines`,
          }),
        ),
        // The graph read backwards, from the symbol already in hand. The
        // Impact page takes a name; this is how somebody who is looking at
        // one gets there without typing it again.
      ),
      // "Incoming", because that is what the list holds. Under a heading
      // reading CALLERS it carried a `defines` edge and a `references` edge,
      // neither of which is a call — the section is every edge that points at
      // this symbol and now says so.
      ends("Incoming", data.callers.map(edge), "Nothing in the graph points at this."),
      ends(
        "Outgoing",
        data.callees.map(edge),
        "A leaf, as far as the extracted edges go.",
        // Targets the store holds no definition for. Left out by default,
        // because a list of names this corpus knows nothing about is noise —
        // and counted, because "no callees" and "every callee is outside the
        // index" are different facts.
        data.hidden
          ? el("button", {
              // A chip, not muted text. At 11px in `--muted` this read as a
              // caption about the list rather than as the control that opens
              // the rest of it, which is the whole of why it was missed.
              class: "chip sm",
              type: "button",
              text: `Show ${data.hidden} unresolved`,
              onclick: async (e) => {
                const button = e.currentTarget;
                button.textContent = "Loading…";
                let all;
                try {
                  all = await api(scoped(`/api/neighbors?name=${encodeURIComponent(node.name)}&all=1`));
                } catch (err) {
                  button.replaceWith(error(err.message));
                  return;
                }
                button.replaceWith(
                  el(
                    "ul",
                    { class: "rail-list nested" },
                    all.unresolved.map((one) =>
                      el(
                        "li",
                        {},
                        el("span", { text: one.name }),
                        el("span", { class: "via", text: one.kind }),
                      ),
                    ),
                  ),
                );
              },
            })
          : null,
      ),
      confidenceLegend(),
    );
    // The two readings of a selected symbol, side by side as the v4 rail has
    // them: the chunks it lives in, and what reaches it.
    //
    // The first of these used to read "Ask the index a question", which is
    // the top bar's wording for the search box — so the rail and the top bar
    // gave one destination two names, which is what finding 3.24 is about.
    // It is the same journey with a name that says what you get.
    fill(
      actions,
      el("button", {
          class: "button secondary small",
          type: "button",
          text: "Chunks it lives in",
          onclick: () => {
            state.pendingQuery = node.name;
            go("search");
          },
        }),
        el("button", {
          class: "button small",
          type: "button",
          text: "Blast radius",
          onclick: () => {
            state.impactSymbol = node.name;
            // The store the symbol was picked in: the same name in another
            // open store is a different symbol with a different reach.
            state.impactStore = node.store || (chosen.size === 1 ? [...chosen][0] : "");
            go("impact");
          },
        }),
    );
    actions.hidden = false;
  }

  /* The store chips scope the canvas, and until 0.24.0 they did not scope the
   * rail: a name defined in two open stores listed both stores' callers under
   * a chip that named one of them. The panel then contradicted the picture
   * beside it, which is worse than either answer on its own. */
  function scoped(path) {
    const query = new URLSearchParams();
    for (const store of chosen) query.append("store", store);
    const tail = query.toString();
    return tail ? `${path}${path.includes("?") ? "&" : "?"}${tail}` : path;
  }

  async function load(params) {
    meta.textContent = "loading…";
    const query = new URLSearchParams(params || {});
    for (const store of chosen) query.append("store", store);
    let data;
    try {
      data = await api(`/api/graph?${query}`);
    } catch (e) {
      meta.textContent = "";
      actions.hidden = true;
      return fill(rail, error(e.message));
    }
    // Stores that did not answer, over the graph the rest of them drew. This
    // page was the whole of issue #129: one unreadable store used to make it
    // draw nothing at all, with a message naming neither the store nor the
    // fact that the others were fine.
    fill(unreadable, unreadableNotice(data.failed));
    if (!data.nodes.length) {
      meta.textContent = "0 symbols";
      canvas.draw({ nodes: [], edges: [] }, kinds);
      return blank(
        data.total === 0
          ? "This store has no symbols yet. The graph is built as files are indexed — run Index once, or leave the daemon watching."
          : "Nothing in this scope. Clear the filters, or pick a store.",
      );
    }
    canvas.draw(data, kinds);
    counts();
    /* Nodes with nothing between them is a state, not a drawing. It happens
     * for a real reason — a language whose parser extracts definitions and no
     * call edges, or a scope narrow enough that both ends of every edge fell
     * outside it — and a field of unconnected boxes with no explanation reads
     * as a broken page. Said rather than drawn silently. */
    if (!data.edges.length) {
      return blank(
        "Nothing in view is connected. Either this scope holds both ends of no " +
          "edge — widen it, or clear it — or the language here is one semlith " +
          "parses for definitions but not yet for calls. The Index page's " +
          "per-language table says which.",
      );
    }
    // A focused view arrives with its centre chosen, so the rail says something
    // before the first click rather than asking for one.
    const centre = (params && params.name) || canvas.best();
    if (centre && canvas.pick(centre)) return;
    blank("Pick a node to see what calls it and what it calls.");
  }

  function focus(name) {
    load({ name, limit: "45" });
  }

  /* Several names, comma-separated: where each is defined and its first
   * line, the table `semlith symbol a b c` prints. The graph centres on the
   * first. */
  const defsCard = el("div", { class: "card table-card graph-defs", hidden: "" });
  async function showDefinitions(names) {
    const params = new URLSearchParams({ names: names.join(",") });
    for (const store of chosen) params.append("store", store);
    let data;
    try {
      data = await api(`/api/symbol?${params}`);
    } catch (_) {
      return;
    }
    const rows = data.table || [];
    defsCard.hidden = false;
    fill(
      defsCard,
      el(
        "div",
        { class: "table-head" },
        el("h2", { text: "Definitions" }),
        el("span", { class: "muted", text: `${rows.length} for ${names.length} name${names.length === 1 ? "" : "s"}` }),
      ),
      el(
        "div",
        { class: "table-wrap" },
        el(
          "table",
          {},
          el("caption", { class: "sr-only", text: "Every definition of the names asked for" }),
          el(
            "thead",
            {},
            el(
              "tr",
              {},
              el("th", { text: "Definition" }),
              el("th", { text: "Kind" }),
              el("th", { text: "Where" }),
              el("th", { text: "First line" }),
            ),
          ),
          el(
            "tbody",
            {},
            rows.length
              ? rows.map((row) =>
                  el(
                    "tr",
                    { class: "defs-row" },
                    el("td", { class: "sym" }, el("code", { text: row.qualified })),
                    el("td", { text: row.kind }),
                    el("td", {
                      class: "where path",
                      title: `${row.path}:${row.start_line}-${row.end_line}`,
                      text: `${shortPath(row.path)}:${row.start_line}`,
                    }),
                    el("td", {}, el("code", { class: "one-line", text: row.signature })),
                  ),
                )
              : el("tr", {}, el("td", { colspan: "4", class: "muted", text: "No definition of any of these names." })),
          ),
        ),
      ),
    );
  }

  /** Apply whatever is in the scope box. */
  function applyScope() {
    const value = scopeInput.value.trim();
    defsCard.hidden = true;
    if (value.includes(",")) {
      const names = value.split(",").map((n) => n.trim()).filter(Boolean).slice(0, 20);
      showDefinitions(names);
      return names.length ? load({ name: names[0] }) : load({});
    }
    if (!value) return load({});
    // A path fragment scopes; anything else is read as a symbol to centre on.
    load(value.includes("/") || value.includes(".") ? { path: value } : { name: value });
  }

  const scopeInput = el("input", {
    type: "search",
    placeholder: "Scope to a path, or find symbols: a, b, c",
    // Every other control on this page applies on a click, so a text field
    // that silently waits for Enter reads as broken. It still takes Enter, and
    // now it also says so and has a button.
    "aria-describedby": "graph-scope-hint",
    onkeydown: (e) => {
      if (e.key !== "Enter") return;
      applyScope();
    },
  });
  const scopeButton = el("button", {
    class: "button secondary small in-field",
    type: "button",
    text: "Scope",
    onclick: () => applyScope(),
  });

  const kindChips = EDGE_KINDS.map((kind) =>
    el("button", {
      class: "chip",
      type: "button",
      "aria-pressed": "true",
      text: kind,
      onclick: (e) => {
        const on = e.currentTarget.getAttribute("aria-pressed") !== "true";
        e.currentTarget.setAttribute("aria-pressed", String(on));
        if (on) kinds.add(kind);
        else kinds.delete(kind);
        canvas.filter(kinds);
        canvas.fit();
        counts();
      },
    }),
  );

  const storeChips = liveStores().map((store) =>
    el("button", {
      class: "chip",
      type: "button",
      "aria-pressed": "false",
      text: store.name,
      onclick: (e) => {
        const on = e.currentTarget.getAttribute("aria-pressed") !== "true";
        e.currentTarget.setAttribute("aria-pressed", String(on));
        if (on) chosen.add(store.name);
        else chosen.delete(store.name);
        load({});
      },
    }),
  );

  const page = el(
    "div",
    { class: "graph-page" },
    unreadable,
    el(
      "div",
      { class: "graph-band" },
      pageHead(
        "Graph",
        "Edges are re-extracted on the same pass that re-embeds a file. Never a stale build artifact.",
        { actions: [el("div", { class: "filters" }, kindChips), fit, reset, pause] },
      ),
      el(
        "div",
        { class: "graph-controls" },
        el(
          "div",
          { class: "graph-scope" },
          icon(ICONS.search, 16),
          labelled("graph-scope", "Scope the graph", scopeInput),
          /* `Enter applies it` was a visible sentence inside the field's own
           * border, which pushed the button off its end. It is still here, and
           * still the thing `aria-describedby` on the input points at — a
           * dangling `aria-describedby` is worse than the sentence was. What
           * changed is that it is read rather than seen: the button beside it
           * says the same thing to anybody looking at the field. */
          el("span", {
            class: "sr-only",
            id: "graph-scope-hint",
            text: "Enter applies it",
          }),
          scopeButton,
        ),
        storeChips.length > 1 ? el("div", { class: "filters" }, storeChips) : null,
      ),
      defsCard,
    ),
    el(
      "div",
      { class: "graph-body" },
      el(
        "div",
        { class: "graph-frame" },
        canvas.node,
        el(
          "div",
          { class: "graph-legend" },
          el("span", { class: "key extracted" }, el("i", {}), "extracted"),
          el("span", { class: "key resolved" }, el("i", {}), "resolved"),
          el("span", { class: "key inferred" }, el("i", {}), "inferred"),
          el("span", { class: "key ambiguous" }, el("i", {}), "ambiguous"),
          el("span", { class: "key selected" }, el("i", {}), "selected"),
        ),
        meta,
      ),
      el("div", { class: "graph-side" }, el("div", { class: "graph-scroll" }, rail, mapPanel()), actions),
    ),
  );

  blank("Pick a node to see what calls it and what it calls.");
  // The canvas has no size until it is in the document.
  setTimeout(async () => {
    canvas.start();
    paintPause();
    const pending = state.pendingSymbol;
    state.pendingSymbol = "";
    if (pending) return load({ name: pending, limit: "45" });
    // Land on the busiest symbol's neighbourhood rather than on everything at
    // once. The whole store drawn at once is honest and unreadable, and a
    // reader arriving at a knot learns nothing.
    try {
      const overview = await api("/api/graph?limit=1");
      const busiest = (overview.nodes || [])[0];
      if (busiest) return focus(busiest.name);
    } catch (_) {
      /* fall through to the overview */
    }
    load({});
  }, 0);
  return page;
}

/** One figure with its label, as the design draws them. */
/* Ring one card in the accent, for the single figure a page exists to show.
 *
 * A wrapper rather than an argument on `stat`, because emphasis is a property
 * of the page's argument and not of the number: the same statistic is ringed on
 * one page and plain on another. */
function ringed(node) {
  node.classList.add("ringed");
  return node;
}

function stat(label, value, note) {
  return el(
    "div",
    { class: "stat" },
    el("span", { class: "eyebrow", text: label }),
    el("span", { class: "value", text: value }),
    note ? el("span", { class: "sub", text: note }) : null,
  );
}

// --------------------------------------------------------------- ledger

async function ledgerView() {
  let data;
  try {
    data = await api("/api/ledger");
  } catch (e) {
    return el("div", { class: "view" }, pageHead("Retrieval ledger"), error(e.message));
  }

  const on = data.recording;
  // Live: the ledger's one record call moves this counter, whichever surface
  // answered the retrieval.
  watchLive(["ledger"], repaintView);
  return el(
    "div",
    { class: "view" },
    pageHead(
      "Retrieval ledger",
      "Every query an agent ran, recorded locally. The honest token number, a debugging trail, and an audit record that never left the machine.",
      {
        pill: on
          ? pill("recording", "on", {
              title:
                "On by default. Rows live in the store beside the chunks and never leave this machine.",
            })
          : pill("not recording", null),
        // The v4 header puts this here, and it is the question the page ends
        // on: having read what the agents retrieved, the next thing a reader
        // wants is that turned into a file somebody else can read.
        actions: [
          el("button", {
            class: "button secondary",
            type: "button",
            text: "Build a report",
            onclick: () => go("reports"),
          }),
        ],
      },
    ),
    // Directly under the head, where the design puts it. It was second from
    // the bottom of the page, so a ledger that had recorded nothing said so
    // after everything it had failed to record.
    on
      ? null
      : el(
          "div",
          { class: "notice" },
          el("div", { class: "what", text: "Recording is off" }),
          el(
            "div",
            {},
            "The daemon was started with ",
            mono("--no-ledger"),
            ". Start it without that flag to record what your agents retrieve. Nothing is sent anywhere; the rows live in the store beside the chunks.",
          ),
        ),
    // Six across, then two wide, then who asked — the order the design puts
    // them in, and the order they are read in: what was asked, what it cost,
    // what it saved, and how much of the ledger that figure covers.
    el(
      "div",
      { class: "strip" },
      stat(
        "Queries recorded",
        n(data.queries),
        `${n(data.clients)} client${data.clients === 1 ? "" : "s"}`,
      ),
      stat("Excerpt tokens", n(data.excerpt_tokens), "what agents were actually sent"),
      stat(
        "Whole-file tokens",
        n(data.whole_file_tokens),
        "what reading those files whole would have cost, counted with the store's tokenizer",
      ),
      // A ratio never stands alone. Coverage says how much of the ledger it is
      // computed over, and the tier says whether the tokens were counted or
      // estimated — without both, a number like 18.3x is a marketing claim.
      // Ringed rather than merely present: it is the figure a reader came for,
      // and the ring is what carries its two qualifiers with it.
      ringed(
        stat(
          "Measured ratio",
          data.ratio ? `${data.ratio.toFixed(1)}×` : "—",
          data.ratio ? `coverage ${data.coverage}% · ${data.tier}` : "needs a recorded query",
        ),
      ),
      stat(
        "Coverage",
        `${data.coverage || 0}%`,
        `${n(data.credited || 0)} of ${n(data.queries)} retrievals credited`,
      ),
      stat(
        "Net tokens",
        n(data.net_tokens || 0),
        "whole-file less excerpt, over rows that found something",
      ),
    ),
    el(
      "div",
      { class: "strip" },
      /* Read from the route rather than derived as queries less credited.
       * Since 0.24.0 an uncredited retrieval is one of two different things —
       * a question semlith could not answer, and a file an agent read whole
       * without asking — and subtracting one number from another counted them
       * as the same thing. */
      /* A share, as the v4 design draws it. `0 of 4` is two numbers a reader
       * has to divide; the percentage is the thing they were dividing for, and
       * the count is kept in the caption so the denominator is never lost. */
      stat(
        "Zero-hit",
        `${share(data.zero_hit || 0, data.queries || 0)}`,
        `${n(data.zero_hit || 0)} of ${n(data.queries)} queries the corpus could not answer — recorded, and credited nothing`,
      ),
      /* The figure the savings claim is defended against: what an agent read
       * whole anyway, on a file this store holds. Measured on a client with the
       * steering hook, a floor everywhere else — and the caption says which,
       * because a floor presented as a count is the flattering half of a
       * number. */
      stat(
        "Refunds",
        `${n(data.refunds || 0)}${data.refunds_measured ? "" : "+"}`,
        data.refunds_measured
          ? "files read whole after all, seen by the steering hook — measured"
          : "a floor: no steering hook reports here, so reads semlith never served are uncounted",
      ),
      stat("Tier", data.tier || "modelled", "modelled · measured — measured when the store's own tokenizer counted it"),
    ),
    clientBreakdown(data.by_client),
    /* From here the page is the design's order, which it was not.
     *
     * The three command cards were at the foot, under everything, where the
     * design puts them sixth — directly after the by-client line and before
     * the tabs. Session replay sat between the session list and the rows
     * table; the design has it last of the cards, after the rows it annotates,
     * which is also the only order in which it means anything. And the
     * recording-off notice was second from the bottom, so a page that was
     * recording nothing said so after everything it had failed to record; it
     * is now directly under the page head. */
    el(
      "div",
      { class: "grid three" },
        el(
          "div",
          { class: "card pad dense" },
          copyField("semlith ledger --last 20"),
          el("p", {
            class: "subtitle",
            text: "Prints the ledger on the command line. The same rows this page shows.",
          }),
        ),
        el(
          "div",
          { class: "card pad dense" },
          copyField("semlith ledger --verify"),
          el("p", {
            class: "subtitle",
            text: "Re-walks the hash chain and names the first row that does not verify.",
          }),
          el("p", {
            class: "subtitle",
            text: data.intact
              ? "The chain is intact."
              : "The chain does not verify. Some rows have been edited or removed.",
          }),
        ),
        el(
          "div",
          { class: "card pad dense" },
          copyField("semlith start --no-ledger"),
          el("p", {
            class: "subtitle",
            text: "Run the daemon without recording, for this session only. SEMLITH_LEDGER=0 does the same for a machine.",
          }),
        ),
    ),
    /* Two tabs rather than three stacked cards, as the v4 design has it.
     *
     * The rows and the replay are two readings of the same ledger, and stacked
     * they made a long page where the second one was found by scrolling past
     * the first. A tab says they are alternatives. The sessions table keeps its
     * own card above them, because it is the summary both tabs are of. */
    ledgerSessions(data),
    ledgerTabs(data),
    says(
      "Stored in ",
      mono("~/.semlith/stores/<name>/store.db"),
      ", table ",
      mono("retrievals"),
      ". On by default; ",
      mono("--no-ledger"),
      " or ",
      mono("SEMLITH_LEDGER=0"),
      " turns it off, and deleting the rows is one ",
      mono("DELETE"),
      ".",
    ),
  );
}

/* The one switch on the Privacy page.
 *
 * Off unless turned on, because what it reads belongs to another program.
 * The row says what would be read and from where before anything is, so the
 * answer to "what does this turn on" is on the page rather than in a doc. */
/* The Privacy page's session replay control.
 *
 * The design draws this as a clickable row rather than as a heading with a
 * button beside it: a 15px square knob, a status line, and the sentence that
 * says what turning it on does, all inside one hit target that changes colour
 * with the state. The button it replaced said `Turn on` and left the reader to
 * infer the state from a paragraph under it.
 *
 * It is not a `checkbox` element because it is not one control in a form; it
 * is the whole row. The ARIA is `switch`, which is what it behaves like, and
 * the keyboard reaches it because it is a `button`.
 */
function sessionReplayToggle() {
  const state_ = { enabled: false, from: "", client: "" };
  const knob = el("span", { class: "knob", "aria-hidden": "true" });
  const status = el("span", { class: "replay-state" });
  const row = el(
    "button",
    { class: "replay-switch", type: "button", role: "switch", "aria-checked": "false", disabled: true },
    knob,
    el(
      "span",
      { class: "replay-switch-text" },
      status,
      el("span", {
        class: "replay-switch-note",
        text: "Session replay reads this machine's agent session logs to confirm what an agent did after a semlith answer. Off by default. Local only. Nothing is uploaded.",
      }),
    ),
  );
  const failure = el("p", { class: "note" });

  function paint() {
    row.disabled = false;
    row.classList.toggle("on", state_.enabled);
    row.setAttribute("aria-checked", String(state_.enabled));
    /* The design's two status lines, verbatim. Where the transcripts are read
     * from is appended when this machine has told us, because `no agent log is
     * opened` is a claim about a directory and a reader may want to know
     * which. */
    status.textContent = state_.enabled
      ? `On · reading local agent logs${state_.from ? ` under ${state_.from}` : ""}`
      : "Off · no agent log is opened";
  }

  row.addEventListener("click", async () => {
    row.disabled = true;
    failure.textContent = "";
    try {
      const answer = await api("/api/ledger/replay", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ on: !state_.enabled }),
      });
      state_.enabled = !!answer.enabled;
    } catch (e) {
      failure.textContent = e.message;
      row.disabled = false;
      return;
    }
    paint();
  });

  (async () => {
    try {
      const data = await api("/api/ledger/replay");
      state_.enabled = !!data.enabled;
      state_.from = data.from || "";
      state_.client = data.client || "";
    } catch (_) {
      /* the row still renders, saying it is off */
    }
    paint();
  })();

  return el(
    "div",
    { class: "card pad" },
    el("div", { class: "card-head" }, el("h2", { text: "Session replay" })),
    row,
    failure,
  );
}

/* Session replay: what the agent did after each answer.
 *
 * Reads this machine's Claude Code transcripts, and only when the Privacy
 * page's toggle is on. Nothing read here is sent anywhere — the files belong
 * to another program, which is exactly why the toggle exists and why the tab
 * says where it read from. */
/** The rows and the replay, as two tabs over one ledger. */
function ledgerTabs(data) {
  const panel = el("div", { class: "tab-panel" });
  const views = [
    ["Retrievals", () => ledgerRows(data)],
    ["Session replay", () => sessionReplay(data)],
  ];
  const buttons = views.map(([label], i) =>
    el("button", {
      class: "tab",
      type: "button",
      text: label,
      "aria-pressed": String(i === 0),
      onclick: () => show(i),
    }),
  );

  function show(index) {
    buttons.forEach((b, i) => b.setAttribute("aria-pressed", String(i === index)));
    fill(panel, views[index][1]());
  }

  show(0);
  return el("div", { class: "rows tight" }, el("div", { class: "tabs" }, buttons), panel);
}

function sessionReplay(ledger) {
  const body = el("div", { class: "replay-body" });
  // `sess-7f21 · Claude Code · 41 queries` in the design: which transcript, the
  // client it belongs to, and how much of it this panel is about.
  const meta = el("span", { class: "replay-meta", text: "reading…" });

  /* The ledger's own row for one question, if it has one.
   *
   * The transcript knows what was asked and what the agent did next; it does
   * not know what the answer cost, because that is semlith's own record. The
   * design prints both on one line, so they are joined here — on the query
   * text, over the rows the Retrievals tab already fetched, newest first. No
   * round trip, and no second reading of the ledger.
   *
   * A query asked twice matches the newer row. That is a join on text rather
   * than on identity: the transcript carries no retrieval id, and minting one
   * to match against would be a change to the protocol for a subtitle.
   */
  const rows = (ledger && ledger.rows) || [];
  function costOf(query) {
    if (!query) return "";
    const row = rows.find((r) => (r.query || "") === query);
    if (!row) return "";
    const hits = row.hits === undefined ? null : row.hits;
    const tokens = row.net_tokens === undefined ? row.tokens : row.net_tokens;
    const parts = [];
    if (hits !== null) parts.push(`${hits} hit${hits === 1 ? "" : "s"}`);
    if (tokens !== undefined && tokens !== null) parts.push(`${n(tokens)} tokens`);
    return parts.join(" · ");
  }

  /** `14:22:08` in the reader's own zone, from the transcript's timestamp. */
  function clockOf(at) {
    if (!at) return "—";
    const when = new Date(at);
    if (Number.isNaN(when.getTime())) return "—";
    return when.toLocaleTimeString(undefined, { hour12: false });
  }

  function answerRow(answer) {
    const cost = costOf(answer.query);
    return el(
      "div",
      { class: "replay-answer" },
      el("span", { class: "at", text: clockOf(answer.at) }),
      el(
        "div",
        { class: "replay-what" },
        el(
          "div",
          { class: "line one" },
          // The question, and beside it what it cost. A call whose input this
          // build does not know how to read has no question, so the tool's own
          // name stands in — once, not in both slots, which printed
          // `semlith_stats semlith_stats`.
          el("span", { class: "q", text: answer.query || answer.tool }),
          el("span", { class: "muted", text: answer.query ? cost || answer.tool : cost }),
        ),
        el("span", { class: `replay-badge ${answer.outcome}`, text: answer.word }),
      ),
    );
  }

  function counts(session) {
    const shown = [
      ["read the whole file", session.refund, "refund"],
      ["grepped anyway", session.miss, "miss"],
      ["edited", session.sufficed, "sufficed"],
      ["nothing recorded", session.unknown, "unknown"],
    ];
    return el(
      "span",
      { class: "replay-counts" },
      shown.map(([label, count, kind]) =>
        count ? el("span", { class: `replay-count ${kind}`, text: `${count} ${label}` }) : null,
      ),
    );
  }

  /* The off state, as the design draws it: one dashed strip, the sentence, and
   * the one button in the prototype that goes to Privacy. The button is here
   * rather than a line of prose telling the reader to find the page, because
   * this panel is where somebody discovers the feature exists. */
  function offPanel() {
    return el(
      "div",
      { class: "replay-off" },
      el("p", {
        text: "Session replay is off. Nothing is read from your agent logs until you turn it on.",
      }),
      el("button", {
        class: "button secondary small",
        type: "button",
        text: "Open Privacy",
        onclick: () => go("privacy"),
      }),
    );
  }

  async function load() {
    let data;
    try {
      data = await api("/api/ledger/replay");
    } catch (e) {
      meta.textContent = "";
      fill(body, error(e.message));
      return;
    }
    const client = data.client === "claude-code" ? "Claude Code" : data.client || "an agent";
    if (!data.enabled) {
      meta.textContent = `off · ${client}`;
      fill(body, offPanel());
      return;
    }
    if (!data.sessions.length) {
      meta.textContent = `on · ${client} · no transcript yet`;
      fill(
        body,
        el("p", {
          class: "subtitle",
          text: `On, and no transcript under ${data.from || "this machine"} holds a semlith call yet.`,
        }),
      );
      return;
    }
    const answers = data.sessions.reduce((sum, session) => sum + session.answers, 0);
    const newest = data.sessions[0];
    meta.textContent = `${newest.id.slice(0, 12)} · ${client} · ${n(answers)} quer${answers === 1 ? "y" : "ies"}`;
    fill(
      body,
      el("p", {
        class: "note",
        text: `Read from ${client} transcripts under ${data.from}${data.skipped ? `, ${data.skipped} older transcript${data.skipped === 1 ? "" : "s"} not read` : ""}.`,
      }),
      data.sessions.map((session) =>
        el(
          "section",
          { class: "replay-row" },
          el(
            "span",
            { class: "line one" },
            el("span", { class: "id", text: session.id.slice(0, 12) }),
            el("span", { class: "muted", text: session.project }),
            el("span", { class: "spacer" }),
            el("span", {
              class: "muted",
              text: `${session.answers} answer${session.answers === 1 ? "" : "s"}`,
            }),
          ),
          counts(session),
          // The timeline the design draws. `recent` is capped in the reader, so
          // a long session says how much of itself is shown rather than
          // silently ending early.
          (session.recent || []).map(answerRow),
          session.answers > (session.recent || []).length
            ? el("p", {
                class: "rail-hint",
                text: `The last ${(session.recent || []).length} of ${session.answers}.`,
              })
            : null,
        ),
      ),
    );
  }

  load();
  return el(
    "div",
    { class: "card pad" },
    el(
      "div",
      { class: "card-head" },
      el("h2", { text: "Session replay" }),
      meta,
    ),
    el("p", {
      class: "subtitle",
      text: "Read from this machine's agent session logs, never sent anywhere. Turn on under Privacy.",
    }),
    body,
  );
}

/* Cost per million tokens, for the one place the ledger turns tokens into
 * money. Input pricing, because a retrieval is what an agent reads.
 *
 * Listed here rather than fetched: the page must work with no network, and a
 * price nobody can see the source of is worse than one written down. */
const MODEL_PRICES = [
  ["Sonnet 5", 3, "sonnet_5"],
  ["Opus 5", 15, "opus_5"],
  ["Haiku 4.5", 1, "haiku_4_5"],
];

/** Hand the viewer a file the page built, without a server round trip. */
function offerDownload(name, text, type) {
  const blob = new Blob([text], { type: `${type};charset=utf-8` });
  const url = URL.createObjectURL(blob);
  const link = el("a", { href: url, download: name });
  document.body.append(link);
  link.click();
  link.remove();
  // Revoked on the next turn of the loop: revoking synchronously races the
  // click in some browsers and the file arrives empty.
  setTimeout(() => URL.revokeObjectURL(url), 0);
}

/** The rows shown, as Markdown, CSV or JSON — the same rows, three ways. */
function exportRows(columns, rows, format, name) {
  const cells = (row) => columns.map(([, read]) => String(read(row) ?? ""));
  const heads = columns.map(([head]) => head);
  if (format === "json") {
    const out = rows.map((row) => {
      const one = {};
      columns.forEach(([head, read]) => {
        one[head] = read(row);
      });
      return one;
    });
    offerDownload(`${name}.json`, JSON.stringify(out, null, 2), "application/json");
    return;
  }
  if (format === "csv") {
    const quote = (v) =>
      v.includes('"') || v.includes(",") || v.includes("\n")
        ? `"${v.split('"').join('""')}"`
        : v;
    const lines = [heads.map(quote).join(","), ...rows.map((row) => cells(row).map(quote).join(","))];
    offerDownload(`${name}.csv`, `${lines.join("\n")}\n`, "text/csv");
    return;
  }
  const lines = [
    `| ${heads.join(" | ")} |`,
    `| ${heads.map(() => "---").join(" | ")} |`,
    ...rows.map((row) => `| ${cells(row).join(" | ")} |`),
  ];
  offerDownload(`${name}.md`, `${lines.join("\n")}\n`, "text/markdown");
}

/* One row per session: who asked, how much they read, and what that would
 * have cost at a price the reader picks.
 *
 * Never a saved figure without its coverage and its tier — a session counted
 * by the four-character fallback is `modelled` and says so in its own row,
 * because averaging it with a measured one would make two different things
 * into one number. */
function ledgerSessions(data) {
  const all = data.sessions || [];
  let client = "";
  let tier = "";
  let price = MODEL_PRICES[0];

  const table = dataTable({
    className: "w-sessions",
    sort: "last",
    dir: "desc",
    rows: [],
    caption: "Every agent session this machine recorded, newest first.",
    columns: [
      {
        key: "last",
        label: "Last seen",
        className: "meta",
        value: (r) => r.last,
        render: (r) => el("span", { class: "one-line", title: r.when, text: r.when }),
      },
      {
        key: "session",
        label: "Session",
        className: "meta",
        render: (r) =>
          el("span", {
            class: "one-line",
            title: r.session || "recorded before sessions were",
            text: r.session ? r.session.slice(0, 12) : "—",
          }),
      },
      { key: "client", label: "Agent", className: "meta", render: (r) => r.client },
      { key: "store", label: "Store", className: "meta narrow-drop", render: (r) => r.store },
      { key: "retrievals", label: "Reads", className: "num", value: (r) => r.retrievals, render: (r) => n(r.retrievals) },
      {
        key: "net_tokens",
        label: "Net tokens",
        className: "num",
        value: (r) => r.net_tokens,
        render: (r) => n(r.net_tokens),
      },
      {
        key: "cost",
        label: "Cost",
        className: "num",
        value: (r) => r.net_tokens,
        render: (r) => `$${((r.net_tokens / 1e6) * price[1]).toFixed(2)}`,
      },
      { key: "tier", label: "Tier", className: "meta", render: (r) => r.tier },
    ],
  });

  function shown() {
    return all.filter(
      (row) => (!client || row.client === client) && (!tier || row.tier === tier),
    );
  }

  function repaint() {
    const rows = shown();
    table.update(rows, rows.length);
    count.textContent = `${rows.length} of ${all.length} session${all.length === 1 ? "" : "s"}`;
  }

  const count = el("span", { class: "muted" });

  const clients = [...new Set(all.map((row) => row.client))].sort();
  const clientPick = el(
    "select",
    {
      class: "chip",
      "aria-label": "Filter by client",
      onchange: (e) => {
        client = e.currentTarget.value;
        repaint();
      },
    },
    el("option", { value: "", text: "every client" }),
    clients.map((one) => el("option", { value: one, text: one })),
  );
  const tierPick = el(
    "select",
    {
      class: "chip",
      "aria-label": "Filter by tier",
      onchange: (e) => {
        tier = e.currentTarget.value;
        repaint();
      },
    },
    el("option", { value: "", text: "every tier" }),
    el("option", { value: "measured", text: "measured" }),
    el("option", { value: "modelled", text: "modelled" }),
  );
  const modelPick = el(
    "select",
    {
      class: "chip",
      "aria-label": "Cost at",
      onchange: (e) => {
        price = MODEL_PRICES[Number(e.currentTarget.value)] || MODEL_PRICES[0];
        repaint();
      },
    },
    MODEL_PRICES.map(([name], i) =>
      el("option", { value: String(i), text: `cost at ${name}` }),
    ),
  );

  // The columns the export writes are the columns on screen, read through the
  // same functions, so a file and the page can never disagree about a row.
  const columns = () => [
    ["when", (r) => r.when],
    ["session", (r) => r.session],
    ["agent", (r) => r.client],
    ["store", (r) => r.store],
    ["reads", (r) => r.retrievals],
    ["zero_hit", (r) => r.zero_hit],
    ["net_tokens", (r) => r.net_tokens],
    [`cost_usd_at_${price[2]}`, (r) => ((r.net_tokens / 1e6) * price[1]).toFixed(2)],
    ["tier", (r) => r.tier],
  ];

  const exports = ["Markdown", "CSV", "JSON"].map((label) =>
    el("button", {
      class: "button secondary small",
      type: "button",
      text: label,
      onclick: () =>
        exportRows(
          columns(),
          shown(),
          label === "Markdown" ? "md" : label.toLowerCase(),
          "semlith-sessions",
        ),
    }),
  );

  repaint();
  return el(
    "div",
    { class: "card pad" },
    el("span", { class: "card-title", text: "Sessions" }),
    el("div", { class: "filters" }, clientPick, tierPick, modelPick, count, el("span", { class: "spacer" }), exports),
    all.length
      ? table.node
      : empty("No session has recorded a retrieval yet."),
    el("p", {
      class: "subtitle",
      text: "Net tokens are whole-file less excerpt, over the reads that found something. A session counted by the four-character fallback is modelled, not measured, and says so in its own row.",
    }),
  );
}

/** The ledger's rows: the same ones `semlith ledger --last 200` prints. */
function ledgerRows(data) {
  const rows = data.rows || [];
  const table = dataTable({
    className: "w-ledger",
    sort: "at",
    dir: "desc",
    rows,
    caption: "Every retrieval recorded on this machine, newest first.",
    columns: [
      {
        key: "at",
        label: "When",
        className: "meta",
        value: (r) => r.at,
        // Local time with the offset, as the server rendered it — the same
        // string the CLI prints. One dataset, one clock: the two surfaces used
        // to disagree by the UTC offset with neither of them saying so.
        render: (r) => el("span", { class: "one-line", title: r.when, text: r.when }),
      },
      { key: "client", label: "Client", className: "meta", render: (r) => r.client },
      { key: "store", label: "Store", className: "meta narrow-drop", render: (r) => r.store },
      {
        key: "query",
        label: "Query",
        className: "path",
        render: (r) => el("span", { class: "one-line", title: r.query, text: r.query }),
      },
      { key: "hits", label: "Hits", className: "num", render: (r) => n(r.hits) },
      {
        key: "ms",
        label: "ms",
        className: "num narrow-drop",
        render: (r) => n(r.ms),
      },
    ],
  });
  table.update(rows, rows.length);
  /* The table, and nothing around it.
   *
   * It was inside a `card pad` headed `The rows`, under a tab already labelled
   * `Retrievals` — a title restating its tab, and a card border immediately
   * inside the panel's own. The table brings its own surface. */
  return el(
    "div",
    { class: "rows tight" },
    rows.length
      ? table.node
      : empty("Nothing recorded yet. A search from any client writes a row here."),
    data.legacy_rows
      ? el("p", {
          class: "subtitle",
          text: "Some rows here are older than the query id, and were written once per open store — figures that include them may count one search several times. They are left as they are: the chain is never rewritten.",
        })
      : null,
  );
}

/* Who the ledger recorded, and how often.
 *
 * The row that says the ledger works. Before 0.15.0 it could only ever read
 * `portal 31`, because the portal's own search box was the only thing that
 * wrote to it; a line with `claude-code` on it is the whole point of the
 * release. */
function clientBreakdown(byClient) {
  const entries = Object.entries(byClient || {}).sort((a, b) => b[1] - a[1]);
  if (!entries.length) return null;
  return el(
    "div",
    { class: "client-breakdown" },
    // Labelled, because a bare run of names and numbers under eight stat cards
    // reads as a caption for the cards rather than as its own fact. The line
    // after it is the fact: these are the client's own names, from the MCP
    // handshake, not a guess made here.
    el("span", { class: "eyebrow sm", text: "by client" }),
    entries.map(([client, count], i) =>
      el(
        "span",
        { class: "client" },
        i ? el("span", { class: "sep", text: "·" }) : null,
        el("span", { class: "who", text: client }),
        el("span", { class: "count", text: n(count) }),
      ),
    ),
    el("span", {
      class: "client-note",
      text: "— MCP, HTTP and CLI, under the client's own name",
    }),
  );
}

/** The stores that are actually there, for anywhere one can be chosen.
 *
 * A registry entry whose directory is missing is still a row on the Stores
 * page — the user has to be able to see it to delete it — but it is not a
 * store anything can be indexed into, searched or drawn. It used to be offered
 * as "add to alpha" in the Index dropdown and as a chip on Search and Graph,
 * which is the same mechanism that put one store's files into another. */
function liveStores() {
  return state.stores.filter((store) => {
    if (store.missing || store.unopened) return false;
    // A store whose every registered root has gone is a store nothing can
    // sensibly be indexed into: the corpus it is about is not on the machine.
    // A store with no roots recorded at all is a different thing — a `--store`
    // directory the registry never saw — and is offered as it always was.
    const roots = store.roots || [];
    return !roots.length || roots.some((root) => root.present);
  });
}

/** Read the store list into `state`, so every view agrees on how many exist.
 *
 * Numbered, like the runs reads. A delete moves the stores counter and the
 * events counter together, so the sidebar and the Stores page each ask at
 * once — and an answer from before the delete arriving after one from after
 * it left the deleted store counted until the next change, which a delete
 * never follows with. */
let storesAsked = 0;
let storesRead = 0;

async function refreshStores() {
  const mine = ++storesAsked;
  try {
    const data = await api("/api/stores");
    if (mine < storesRead) return state.stores;
    storesRead = mine;
    state.stores = data.stores || [];
    // Kept on the state rather than drawn here, because this runs before every
    // view and the view decides where a notice belongs on its own page.
    state.failed = data.failed || [];
    paintStoreCount();
  } catch (_) {
    // A failed refresh must not empty the list: `state.stores.length` decides
    // whether the app or the first-run screen is shown, and a dropped request
    // is not the same as having no stores.
  }
  return state.stores;
}

// ---------------------------------------------------------------- stores

/* "Agents connected", on the Stores page.
 *
 * The same list the Agents page draws, read from the same route, because two
 * readings of "which agents are talking to this daemon" is how one page says
 * two and the other says none. */
function agentsCard() {
  const card = el("div", { class: "card pad dense" });
  const body = el("div", { class: "rows" });
  fill(
    card,
    el("span", { class: "card-title", text: "Agents connected" }),
    body,
    el("span", { class: "spacer" }),
    // A link rather than a button box: in the design this is a line of text
    // at the foot of the card, and as a ghost button stretched by the flex
    // column it read as a centred banner across the bottom of the card.
    el("button", {
      class: "link-button",
      type: "button",
      text: "Copy config for another client",
      onclick: () => go("agents"),
    }),
  );
  fill(body, skeletonRows(3));

  api("/api/agents")
    .then((data) => {
      noteAgents(data);
  // Live: a client connecting, and every tool call it makes, moves the clients
  // counter, so the table fills in as agents talk rather than on a reload.
  watchLive(["clients"], repaintView);
      const live = data.connections || [];
      if (!live.length) {
        fill(
          body,
          el("div", {
            class: "rail-hint",
            text: "No client is talking to this daemon right now.",
          }),
        );
        return;
      }
      fill(
        body,
        live.map((client) =>
          el(
            "div",
            { class: "kv" },
            el("span", { text: client.name }),
            el("span", { class: "spacer" }),
            el("span", {
              class: "meta",
              text: `${client.transport} · ${n(client.queries)} quer${
                client.queries === 1 ? "y" : "ies"
              }`,
            }),
          ),
        ),
      );
    })
    .catch((e) => fill(body, error(e.message)));

  return card;
}

/* A question that has to be answered before anything else happens.
 *
 * A native `<dialog>` rather than a div: the focus trap, the Escape key, the
 * inert background and the backdrop are the platform's, and every one of them
 * is a thing a hand-rolled overlay gets wrong. `run` returns a promise; while
 * it is pending the dialog says so, and an error is shown inside it rather
 * than behind it. */
/* A confirm dialog. `lead` stays under the title and `tail` above the
 * buttons; only `extra` between them scrolls, so a long list of findings
 * never takes the file it is about, or the choice, off the screen. */
function ask({ title, body, extra, lead, tail, confirm, tone, run, wide }) {
  const dialog = el("dialog", { class: wide ? "modal wide" : "modal" });
  const problem = el("div", { class: "note" });
  const go = el("button", {
    class: tone === "bad" ? "button danger" : "button",
    type: "button",
    text: confirm,
    onclick: async () => {
      go.disabled = true;
      const was = go.textContent;
      go.textContent = "Working…";
      problem.className = "note";
      problem.textContent = "";
      try {
        await run();
        dialog.close();
      } catch (e) {
        problem.className = "note bad";
        problem.textContent = e.message;
        go.disabled = false;
        go.textContent = was;
      }
    },
  });
  fill(
    dialog,
    el("div", { class: "modal-head" }, el("h2", { class: "card-title", text: title }), el("p", { class: "subtitle", text: body }), lead || null),
    extra ? el("div", { class: "modal-body" }, extra) : null,
    el("div", { class: "modal-foot" }, tail || null, problem),
    el(
      "div",
      { class: "actions" },
      el("span", { class: "spacer" }),
      el("button", {
        class: "button secondary",
        type: "button",
        text: "Cancel",
        onclick: () => dialog.close(),
      }),
      go,
    ),
  );
  // Removed on close, however it was closed — the button, Escape, or the
  // backdrop — so the page never accumulates dialogs nobody can see.
  dialog.addEventListener("close", () => dialog.remove());
  dialog.addEventListener("click", (e) => {
    if (e.target === dialog) dialog.close();
  });
  document.body.append(dialog);
  dialog.showModal();
  return dialog;
}

/** Ask before deleting a store.
 *
 * A store is minutes of embedding, so the question says what goes and what
 * does not, and the button that answers it is the red one. */
function confirmDelete(name) {
  ask({
    title: `Delete ${name}?`,
    body: "Its vectors, chunks, graph and ledger are deleted, and the registry stops listing it. The files it indexed are untouched.",
    confirm: `Delete ${name}`,
    tone: "bad",
    run: async () => {
      const done = await post("/api/store/delete", { store: name });
      await refreshStores();
      // After the re-render, not before: the render replaces the element the
      // message would have been written into.
      await render();
      note(done.message);
    },
  });
}

/** A line under the page head, for something that just happened to the page. */
function note(text, bad) {
  const holder = document.querySelector(".view > .note.page-note");
  if (!holder) return;
  holder.className = bad ? "note page-note bad" : "note page-note";
  holder.textContent = text;
}

/* The stores ticked for a bulk delete, by name. Outside the view, because the
 * view is redrawn whenever a store's counters move, and a selection that
 * vanished under a watcher's re-embed would be one nobody could trust. */
const storesPicked = new Set();

async function storesView() {
  const stores = await refreshStores();
  // A store deleted elsewhere is no longer selectable.
  for (const name of storesPicked) {
    if (!stores.some((s) => s.name === name)) storesPicked.delete(name);
  }
  /* Live. A store the CLI just made, a watcher re-embed, a run's own note —
   * each moves a counter the shared poll is watching, and this page redraws
   * from the route it already reads rather than from a timer of its own.
   * Deliberately not on `runs`: that counter moves on every file of every run,
   * and a table that rebuilt forty times a second would be unreadable. */
  watchLive(["stores", "events"], repaintView);

  const rootNote = el("div", { class: "note" });
  /* A root the registry lists and the disk no longer has. The picker is the
   * same one the page already uses, and re-pointing is a registry edit: no
   * re-embedding, nothing rewritten. */
  const rootPicker = folderPicker({
    choose: "Point the store here",
    onError: (message) => {
      rootNote.className = "note bad";
      rootNote.textContent = message;
    },
    onChoose: async (path) => {
      rootNote.className = "note";
      rootNote.textContent = "Re-pointing…";
      try {
        const done = await post("/api/root", { store: rootPicker.store, root: path });
        rootNote.textContent = done.message;
        await refreshStores();
      } catch (e) {
        rootNote.className = "note bad";
        rootNote.textContent = e.message;
      }
    },
  });

  function repoint(store, missing) {
    rootPicker.store = store;
    rootNote.className = "note";
    rootNote.textContent = `Choose where ${missing.split("/").pop() || missing} lives now.`;
    rootPicker.open("");
  }

  const adoptNote = el("div", { class: "note" });
  const picker = folderPicker({
    choose: "Adopt this directory",
    onError: (message) => {
      adoptNote.className = "note bad";
      adoptNote.textContent = message;
    },
    onChoose: async (path) => {
      adoptNote.className = "note";
      adoptNote.textContent = "Adopting…";
      try {
        const done = await post("/api/adopt", { path });
        adoptNote.textContent =
          done.message || `${path} is registered. It joins on the next daemon start.`;
        await refreshStores();
      } catch (e) {
        adoptNote.className = "note bad";
        adoptNote.textContent = e.message;
        // Back where it failed, rather than closed. A failed adopt used to
        // drop the reader onto the Stores page, and reopening the picker
        // started again at the home directory with every step of navigation
        // lost.
        picker.open(path);
      }
    },
  });

  const head = pageHead("Stores", "Everything indexed on this machine. Nothing leaves it.", {
    actions: [
      el(
        "button",
        { class: "button secondary", type: "button", onclick: () => picker.open("") },
        icon(ICONS.folder),
        "Adopt existing .semlith",
      ),
      el(
        "button",
        { class: "button", type: "button", onclick: () => go("index") },
        icon(ICONS.plus),
        "Index a folder",
      ),
    ],
  });

  if (!stores.length) {
    return el(
      "div",
      { class: "view" },
      head,
      picker.node,
      adoptNote,
      empty("No store is open. Index a folder and it appears here."),
    );
  }

  const totals = stores.reduce(
    (sum, s) => ({
      files: sum.files + s.files,
      chunks: sum.chunks + s.chunks,
      bytes: sum.bytes + s.bytes,
      lines: sum.lines + (s.lines || 0),
      watching: sum.watching + (s.watching ? 1 : 0),
      formats: Math.max(sum.formats, s.formats || 0),
      readers: Math.max(sum.readers, s.readers || 0),
    }),
    { files: 0, chunks: 0, bytes: 0, lines: 0, watching: 0, formats: 0, readers: 0 },
  );
  const dim = stores.find((s) => s.dim)?.dim;

  /* The row the v4 design puts at the foot of the stores card.
   *
   * It is the question the table raises and does not answer: a row says a
   * store holds 4 180 chunks, and this is where the reader goes to find out
   * what is in them. Inside the card rather than under it, because the design
   * draws it as the card's last row and a button floating below the border
   * reads as belonging to the next block. */
  const insideLink = el(
    "button",
    { class: "table-follow", type: "button", onclick: () => go("corpus") },
    el("span", { text: "See what is actually inside the index" }),
    icon(ICONS.arrowRight, 15),
  );

  // The Files page's bulk bar, for stores: one confirm that names each one.
  const bulkBar = el("div", { class: "bulk", hidden: true });
  function paintBulk() {
    bulkBar.hidden = storesPicked.size === 0;
    if (!storesPicked.size) return;
    const names = [...storesPicked].sort();
    const many = `${n(names.length)} store${names.length === 1 ? "" : "s"}`;
    fill(
      bulkBar,
      el("span", { class: "meta", text: `${many} selected` }),
      el("span", { class: "spacer" }),
      el("button", {
        class: "button secondary small",
        type: "button",
        text: "Clear",
        onclick: () => {
          storesPicked.clear();
          for (const box of table.node.querySelectorAll("input.pick")) box.checked = false;
          paintBulk();
        },
      }),
      el("button", {
        class: "button danger small",
        type: "button",
        text: `Delete ${many}`,
        onclick: () =>
          ask({
            title: `Delete ${many}?`,
            body: `${names.join(", ")}: their vectors, chunks, graph and ledger are deleted, and the registry stops listing them. The files they indexed are untouched.`,
            confirm: `Delete ${many}`,
            tone: "bad",
            run: async () => {
              const done = await post("/api/store/delete", { stores: names });
              for (const name of done.deleted || []) storesPicked.delete(name);
              await refreshStores();
              await render();
              note(done.message, (done.failed || []).length > 0);
            },
          }),
      }),
    );
  }

  const table = dataTable({
    className: "w-stores",
    remember: "stores",
    caption: "Every store on this machine: where it is, what it holds, and when it was last written to.",
    sort: "name",
    rows: stores,
    columns: [
      {
        key: "pick",
        label: "",
        className: "pick",
        sortable: false,
        head: () => {
          const all = el("input", {
            type: "checkbox",
            class: "pick all",
            "aria-label": "Select every store on this page",
            onchange: () => {
              for (const box of table.node.querySelectorAll("tbody input.pick")) {
                if (box.checked !== all.checked) box.click();
              }
            },
          });
          return all;
        },
        render: (s) => {
          const box = el("input", {
            type: "checkbox",
            class: "pick",
            "aria-label": `Select ${s.name}`,
            onchange: () => {
              if (box.checked) storesPicked.add(s.name);
              else storesPicked.delete(s.name);
              paintBulk();
            },
          });
          box.checked = storesPicked.has(s.name);
          return box;
        },
      },
      {
        key: "name",
        label: "Store",
        value: (s) => s.name,
        render: (s) =>
          el(
            "div",
            { class: "rows tight" },
            lineCell(s.name, "name"),
            pathCell(s.dir, "meta"),
            // A registry entry whose directory is not there. Its own badge,
            // naming the path that is absent, rather than a word in the "last
            // write" column where it is not a last write.
            s.missing
              ? el(
                  "div",
                  { class: "chips" },
                  pill("missing", "bad", { title: `nothing at ${s.dir}` }),
                  el("span", {
                    class: "meta",
                    text: `nothing at ${s.dir} — delete the entry, or put the store back`,
                  }),
                )
              : null,
            // A store whose corpus has gone. It opens, it answers, and what it
            // answers about is not on disk any more — which is a different
            // fault from a registry entry with no store, and was said nowhere
            // but inside the ROOTS column that a phone drops.
            !s.missing && (s.roots || []).length && !(s.roots || []).some((r) => r.present)
              ? el(
                  "div",
                  { class: "chips" },
                  pill("root missing", "bad", {
                    title: (s.roots || []).map((r) => r.path).join(", "),
                  }),
                  el("span", {
                    class: "meta",
                    text: `nothing at ${(s.roots || []).map((r) => r.path).join(", ")} — re-point it, or delete the store`,
                  }),
                )
              : null,
            // What the daemon reconciled away when it opened this store: rows
            // it held for files outside every root it is registered against.
            s.pruned
              ? el(
                  "div",
                  { class: "chips" },
                  pill(`${n(s.pruned)} reconciled`, "warn"),
                  el("span", {
                    class: "meta",
                    text: "files it held from outside its roots, dropped when this daemon opened it. The files on disk are untouched.",
                  }),
                )
              : null,
            // The root on a phone, where the ROOTS column is dropped and
            // nothing else said what the store indexes. Truncated from the
            // front like every other path: at 390px a plain span broke a home
            // directory into eight-character pieces down the cell.
            pathCell((s.roots || []).map((r) => r.path).join(", ") || "no roots", "meta only-narrow"),
            // The model per store, which the About page states only once for
            // the machine. Two stores can have been built with two models and
            // their vectors are not comparable, so it belongs beside the row.
            lineCell(`${s.model} · ${s.dim} dims`, "meta"),
            // Two things worth saying about a store rather than about its
            // contents: whether this machine has agreed to open it without
            // being told to, and whether anybody else on the machine can read
            // what it holds.
            s.trusted === false
              ? el(
                  "div",
                  { class: "chips" },
                  pill("not trusted", "warn"),
                  el("button", {
                    class: "button secondary small",
                    type: "button",
                    text: "Trust this store",
                    onclick: async (e) => {
                      const button = e.currentTarget;
                      button.disabled = true;
                      adoptNote.className = "note";
                      adoptNote.textContent = "Trusting…";
                      try {
                        await post("/api/trust", { path: s.dir });
                        adoptNote.textContent = `${s.dir} is trusted. semlith opens it from its own directory now, without --store.`;
                        await refreshStores();
                      } catch (err) {
                        adoptNote.className = "note bad";
                        adoptNote.textContent = err.message;
                        button.disabled = false;
                      }
                    },
                  }),
                )
              : null,
            s.loose_mode
              ? el(
                  "div",
                  { class: "chips" },
                  pill(`mode ${s.loose_mode}`, "warn"),
                  el("span", {
                    class: "meta",
                    text: "other users on this machine can read what this store indexed; the next open narrows it to 700",
                  }),
                )
              : null,
          ),
      },
      {
        key: "roots",
        label: "Roots",
        className: "meta narrow-drop",
        sortable: false,
        render: (s) =>
          (s.roots || []).length
            ? s.roots.map((r) =>
                r.present
                  ? pathCell(r.path)
                  : el(
                      "div",
                      { class: "gone-row" },
                      pathCell(r.path, "gone"),
                      // A root the registry still lists and the disk no longer
                      // has. Re-pointing it is one click rather than a command
                      // with two paths in it.
                      el("button", {
                        class: "button danger small",
                        type: "button",
                        text: "Re-point",
                        onclick: () => repoint(s.name, r.path),
                      }),
                    ),
              )
            : "—",
      },
      { key: "files", label: "Files", className: "num", value: (s) => s.files, render: (s) => n(s.files) },
      {
        key: "chunks",
        label: "Chunks",
        className: "num narrow-drop",
        value: (s) => s.chunks,
        render: (s) => n(s.chunks),
      },
      {
        /* One line per store, and never the number on its own: the tokens, the
         * share of retrievals they are computed over, and how they were
         * counted, in one cell. A saved-token figure without its coverage and
         * its tier is a figure a reader is being asked to take on trust, which
         * is what made the 0.17.2 README paragraph unshippable. No chart: a
         * chart of one number is decoration. */
        key: "saved",
        label: "Saved",
        className: "num narrow-drop",
        value: (s) => (s.savings ? s.savings.net_tokens : -1),
        render: (s) => {
          if (!s.savings || !s.savings.total) {
            return el("span", { class: "meta", text: "—" });
          }
          return el(
            "span",
            {
              class: "meta",
              title: `${n(s.savings.net_tokens)} tokens saved over ${n(s.savings.credited)} of ${n(s.savings.total)} retrievals, counted ${s.savings.tier}`,
            },
            `${n(s.savings.net_tokens)} · ${s.savings.coverage}% · ${s.savings.tier}`,
          );
        },
      },
      {
        key: "last_write",
        label: "Last write",
        value: (s) => s.last_write || 0,
        // Green means a write landed. A store that is watched and has never
        // been written to is neither good news nor a warning, so it carries a
        // plain pill with no dot at all.
        render: (s) => {
          // Registered, and this daemon does not have it open — another
          // process is writing it. One vocabulary in this column: every value
          // here is a last write or the reason there is none to read, and a
          // store's own state lives in its own badge beside its name.
          // One vocabulary: either a time, or the em dash that means there is
          // no time to show. Why there is none — the store is missing, or it
          // was never written to — lives in its own badge beside the name,
          // where it is a fact about the store rather than about this column.
          // Read from the store rather than from this daemon's memory of its
          // own session, so a store written yesterday no longer says "never".
          return s.last_write
            ? pill(when(s.last_write), "good")
            : el("span", { class: "meta", text: "—" });
        },
      },
      {
        key: "open",
        label: "",
        className: "row-end",
        sortable: false,
        // One control rather than a button per action: the destructive one
        // does not belong a mis-aimed click away from the ordinary one.
        render: (s) =>
          rowMenu(() => [
            // With the store, so the page it opens is about the row the menu
            // was opened from. Without it, "Open in Files" from any row — even
            // a dead one — showed every store's files with another store's
            // rows at the top.
            {
              label: "Open in Files",
              onclick: () => {
                state.pendingStore = s.name;
                go("files");
              },
            },
            {
              label: "Delete store…",
              tone: "bad",
              onclick: () => confirmDelete(s.name),
            },
          ]),
      },
    ],
  });

  const feed = stores
    .flatMap((s) => (s.events || []).map((e) => ({ at: e.at, text: e.text })))
    .sort((a, b) => b.at - a.at)
    .slice(0, 40);

  paintBulk();
  return el(
    "div",
    { class: "view" },
    head,
    el(
      "div",
      { class: "strip" },
      stat("Stores", n(stores.length), `${totals.watching} being watched`),
      stat("Files", n(totals.files), `${n(totals.formats)} formats`),
      stat("Chunks", n(totals.chunks), dim ? `${dim}-dimension vectors` : "embedded and searchable"),
      // The caption describes this tile's own number, as every other one does.
      // "11 readers in use" is a fact about format handlers, not about lines.
      stat(
        "Lines",
        n(totals.lines),
        `across ${n(totals.files)} file${totals.files === 1 ? "" : "s"}, read by ${n(
          totals.readers,
        )} reader${totals.readers === 1 ? "" : "s"}`,
      ),
      stat("On disk", bytes(totals.bytes), "int8 quantised"),
    ),
    bulkBar,
    // Above the table, so what a delete did is said where the reader is
    // looking; at the foot of the page it was below the fold.
    el("div", { class: "note page-note", role: "status", "aria-live": "polite" }),
    el(
      "div",
      { class: "scroller" },
      appended(table.node, insideLink),
      el(
        "div",
        { class: "grid fill" },
        el(
          "div",
          { class: "card pad dense" },
          el(
            "div",
            { class: "head" },
            el("span", { class: "card-title", text: "Watcher" }),
            el("span", { class: "meta", text: "live" }),
          ),
          feed.length
            ? el(
                "div",
                { class: "feed" },
                // One line per event, with the whole of it on the tooltip. A
                // line that wraps is three lines when it names a path, and
                // forty of those grew the card until it pushed the table it
                // sits under off the page.
                feed.map((e) =>
                  el(
                    "div",
                    { class: "row" },
                    // The whole stamp on the element as well as in it: the
                    // zone offset is what makes this line to the second, and a
                    // phone has no room for both the time and the offset.
                    el("span", {
                      class: "at",
                      text: clock(e.at),
                      title: clock(e.at),
                    }),
                    lineCell(e.text, "what"),
                  ),
                ),
              )
            : empty("Nothing has changed on disk since the daemon started."),
        ),
        agentsCard(),
      ),
    ),
    /* The four ways into this page, after the table they act on.
     *
     * They stood between the page head and the stat strip, which put four
     * stacked blocks of picker between a reader and the thing they came for.
     * The design has the two affordances they cover — `Adopt existing
     * .semlith` and `Index a folder` — as two buttons in the header row, and
     * those are still there; these are the panels they open. */
    picker.node,
    adoptNote,
    rootPicker.node,
    rootNote,
  );
}

// ----------------------------------------------------------------- files

/** The most rows `/api/files` returns in one request. The route refuses a
 * larger `limit` rather than clamping it. */
const FILES_PER_REQUEST = 500;

/** The most files "select all N matches" will gather.
 *
 * The same ceiling the route puts on `offset`: past it there are no more pages
 * to ask for, so the answer is to narrow the filter — said, rather than a
 * button that quietly acts on a prefix of what it named. */
const FILES_SELECTABLE = 10_000;

async function filesView() {
  const chosenExt = new Set();
  const summary = el("span", { class: "pill" });
  const holder = el("div", {});
  const unreadable = el("div", { class: "unreadable-slot" });
  /* Announced rather than only drawn: a Forget made the row disappear and said
   * nothing anywhere a screen reader would hear it. */
  const announcer = el("div", { class: "sr-only", role: "status", "aria-live": "polite" });
  /* Whether the last empty result was empty because a file was forgotten, so
   * the empty state can say what happened instead of blaming the filter. */
  let justForgot = false;
  /* Which store's files to show. There used to be no way anywhere in the
   * portal to look at one store's files: the Store column did not sort, the
   * page had no store control, and "Open in Files" on a store row navigated
   * here and applied no filter at all. */
  const storeFilter = storePicker(state.pendingStore || "", () => load(true));
  state.pendingStore = "";
  const footnote = el("p", {
    class: "subtitle",
    text: "Forget drops the file's chunks and vectors. The file on disk is untouched.",
  });

  const pathInput = el("input", {
    type: "text",
    placeholder: "path glob, e.g. src/**",
    oninput: () => {
      clearTimeout(load.timer);
      load.timer = setTimeout(() => load(true), 200);
    },
  });

  /** Which request is current, so a slow one cannot overwrite a fast one. */
  let generation = 0;

  async function forget(path, row, store) {
    try {
      // Named, always. The row knows which store holds the file, and a daemon
      // serving two stores cannot guess — it refuses a write that names none,
      // which is what the page used to send.
      await post("/api/forget", { path, store });
      // The row goes, and the page is reloaded behind it: with the server
      // paging, the row that moves up into the gap is on the server.
      row.remove();
      picked.delete(path);
      paintBulk();
      justForgot = true;
      announcer.textContent = `${path} forgotten. Its chunks and vectors are gone from ${store}; the file on disk is untouched.`;
      load();
    } catch (e) {
      // In the note, not over the table: an error that replaces the rows
      // takes away the thing the reader was working with. Rethrown so a
      // dialog waiting on this stays open and shows it too.
      bulkNote.className = "note bad";
      bulkNote.textContent = e.message;
      throw e;
    }
  }

  /* The rows ticked for a bulk forget, by path.
   *
   * Kept out here rather than in the table, because the table re-renders on
   * every sort, page and filter and a selection that vanished when you sorted
   * would be a selection nobody could trust. */
  const picked = new Map();
  // Announced as well as drawn: this is where a bulk Forget says what it did,
  // and a row vanishing is not something a screen reader hears.
  const bulkNote = el("div", { class: "note", role: "status", "aria-live": "polite" });
  const bulkBar = el("div", { class: "bulk", hidden: true });

  function paintBulk() {
    bulkBar.hidden = picked.size === 0;
    if (!picked.size) return;
    const many = picked.size;
    fill(
      bulkBar,
      el("span", {
        class: "meta",
        text: `${n(many)} file${many === 1 ? "" : "s"} selected`,
      }),
      // The header checkbox selects the page, honestly and deliberately. This
      // is how the whole result set is reached, which before this there was no
      // way to do at all. Bounded by what one request returns: past that the
      // answer is to narrow the filter, said rather than a button that quietly
      // acts on a prefix.
      matches > many
        ? matches <= FILES_SELECTABLE
          ? el("button", {
              class: "button secondary small",
              type: "button",
              text: `Select all ${n(matches)} matches`,
              onclick: () => selectEveryMatch(),
            })
          : el("span", {
              class: "meta",
              text: `${n(matches)} files match — narrow the filter to ${n(
                FILES_SELECTABLE,
              )} or fewer to select them all`,
            })
        : null,
      el("span", { class: "spacer" }),
      el("button", {
        class: "button secondary small",
        type: "button",
        text: "Clear",
        onclick: () => {
          picked.clear();
          for (const box of table.node.querySelectorAll("input.pick")) box.checked = false;
          paintBulk();
        },
      }),
      el("button", {
        class: "button danger small",
        type: "button",
        text: `Forget ${n(many)} file${many === 1 ? "" : "s"}`,
        onclick: (e) => {
          const button = e.currentTarget;
          const stores = [...new Set(picked.values())];
          ask({
            title: `Forget ${n(many)} file${many === 1 ? "" : "s"}?`,
            body: `Their chunks and vectors are dropped from ${stores.join(
              " and ",
            )}. The files on disk are untouched, and indexing the folder again brings them back.`,
            confirm: `Forget ${n(many)} file${many === 1 ? "" : "s"}`,
            tone: "bad",
            run: () => forgetPicked(button),
          });
        },
      }),
    );
  }

  async function forgetPicked(button) {
    button.disabled = true;
    button.textContent = "Forgetting…";
    bulkNote.className = "note";
    bulkNote.textContent = "";
    // One call per store: a write names the store it is for, and a selection
    // made across two of them is two writes, not an ambiguous one.
    const byStore = new Map();
    for (const [path, store] of picked) {
      if (!byStore.has(store)) byStore.set(store, []);
      byStore.get(store).push(path);
    }
    try {
      let files = 0;
      let forgot = 0;
      let absent = 0;
      for (const [store, paths] of byStore) {
        const done = await post("/api/forget", { paths, store });
        // One path in a store answers in the single-file shape.
        files += done.files === undefined ? 1 : done.files;
        forgot += done.forgot || 0;
        absent += (done.not_indexed || []).length;
      }
      picked.clear();
      justForgot = true;
      paintBulk();
      bulkNote.textContent = absent
        ? `${n(files)} file${files === 1 ? "" : "s"} forgotten, ${n(forgot)} chunk${
            forgot === 1 ? "" : "s"
          } removed. ${n(absent)} were not indexed and were left alone.`
        : `${n(files)} file${files === 1 ? "" : "s"} forgotten, ${n(forgot)} chunk${
            forgot === 1 ? "" : "s"
          } removed.`;
      load();
    } catch (e) {
      bulkNote.className = "note bad";
      bulkNote.textContent = e.message;
      button.disabled = false;
      paintBulk();
      // Rethrown so the dialog that asked stays open and shows it too.
      throw e;
    }
  }

  const table = dataTable({
    className: "w-files",
    caption: "Every indexed file, the reader that parsed it, and what it contributed to the store.",
    server: true,
    sort: "path",
    columns: [
      {
        key: "pick",
        label: "",
        className: "pick",
        sortable: false,
        // Selects every row on the page rather than every row in the store: a
        // filter of ten thousand files behind one tick is a mistake nobody
        // meant to make.
        head: () => {
          const all = el("input", {
            type: "checkbox",
            class: "pick all",
            "aria-label": "Select every file on this page",
            onchange: () => {
              for (const box of table.node.querySelectorAll("tbody input.pick")) {
                if (box.checked !== all.checked) box.click();
              }
            },
          });
          return all;
        },
        render: (f) => {
          const box = el("input", {
            type: "checkbox",
            class: "pick",
            "aria-label": `Select ${f.path}`,
            onchange: () => {
              if (box.checked) picked.set(f.path, f.store);
              else picked.delete(f.path);
              paintBulk();
            },
          });
          box.checked = picked.has(f.path);
          return box;
        },
      },
      {
        key: "path",
        label: "Path",
        className: "path",
        // One line, under the store's root, with the whole path on the
        // shared tooltip: the machine's absolute path took the column's width
        // and truncated to the part every row had in common.
        render: (f) => {
          const cell = pathCell(rootRel(f.path, f.store));
          cell.title = f.path;
          cell.dataset.tip = f.path;
          return cell;
        },
      },
      {
        key: "store",
        label: "Store",
        className: "meta narrow-drop",
        render: (f) => f.store,
      },
      {
        key: "reader",
        label: "Read as",
        className: "narrow-drop",
        render: (f) => el("span", { class: "tag", text: f.reader }),
      },
      {
        key: "lang",
        label: "Language",
        className: "meta narrow-drop",
        render: (f) => f.lang || "—",
      },
      {
        key: "lines",
        label: "Lines",
        className: "num",
        // An image has no lines. Its pixel size goes here rather than a zero,
        // which would read as a file that failed to parse.
        render: (f) => (f.reader === "image" ? "—" : n(f.lines)),
      },
      { key: "chunks", label: "Chunks", className: "num narrow-drop", render: (f) => n(f.chunks) },
      {
        key: "indexed",
        label: "Indexed",
        className: "meta",
        render: (f) => when(f.indexed_at),
      },
      {
        key: "forget",
        label: "",
        sortable: false,
        render: (f) => {
          const button = el("button", {
            class: "forget",
            type: "button",
            text: "Forget",
            onclick: () => {
              const row = button.closest("tr");
              ask({
                title: "Forget this file?",
                body: `${f.path} — its chunks and vectors are dropped from ${f.store}. The file on disk is untouched, and indexing the folder again brings it back.`,
                confirm: "Forget it",
                tone: "bad",
                run: async () => {
                  button.disabled = true;
                  button.textContent = "Forgetting…";
                  await forget(f.path, row, f.store);
                },
              });
            },
          });
          return button;
        },
      },
    ],
    rows: [],
    total: 0,
    onChange: () => load(),
  });

  async function load(reset) {
    const mine = ++generation;
    const view = table.query();
    if (reset) view.page = 1;
    const params = new URLSearchParams();
    if (pathInput.value.trim()) params.set("path", pathInput.value.trim());
    for (const ext of chosenExt) params.append("ext", ext);
    for (const store of storeFilter.stores()) params.append("store", store);
    params.set("sort", view.sort || "path");
    params.set("dir", view.dir);
    params.set("limit", String(view.perPage));
    params.set("offset", String((view.page - 1) * view.perPage));

    let data;
    try {
      data = await api(`/api/files?${params}`);
    } catch (e) {
      if (mine !== generation) return;
      fill(holder, el("div", { class: "card pad" }, error(e.message)));
      summary.textContent = "";
      return;
    }
    if (mine !== generation) return;
    fill(unreadable, unreadableNotice(data.failed));

    summary.textContent = `${n(data.total)} files · ${n(data.formats)} format${
      data.formats === 1 ? "" : "s"
    } · ${n(data.stores)} store${data.stores === 1 ? "" : "s"}`;

    if (!data.total) {
      // Which of the two empty states this is. "The filter, not the corpus" is
      // right for a filter miss and exactly wrong immediately after a Forget,
      // where the corpus is precisely why there is nothing.
      const because = justForgot
        ? "Nothing left under this filter — the file that matched it has been forgotten."
        : "Nothing indexed matches that. The filter, not the corpus — clear it and look again.";
      fill(holder, el("div", { class: "card pad" }, empty(because)));
      // No rows means no Forget buttons, and a footnote about a button that is
      // not on the page is a footnote about nothing.
      footnote.hidden = true;
      justForgot = false;
      return;
    }
    footnote.hidden = false;
    justForgot = false;
    table.update(data.files, data.total);
    if (!holder.contains(table.node)) fill(holder, table.node);
    matches = data.total;
  }

  /** How many files the current filter matches, for "select all N". */
  let matches = 0;

  /** Tick every file the current filter matches, not just the page.
   *
   * Paged, because one request returns at most `FILES_PER_REQUEST` rows. The
   * header checkbox selects the page, honestly and deliberately; this is how
   * the whole result set is reached, which before this there was no way to do
   * at all. */
  async function selectEveryMatch() {
    bulkNote.className = "note";
    bulkNote.textContent = `Selecting ${n(matches)} files…`;
    for (let offset = 0; offset < matches && offset < FILES_SELECTABLE; offset += FILES_PER_REQUEST) {
      const params = new URLSearchParams();
      if (pathInput.value.trim()) params.set("path", pathInput.value.trim());
      for (const ext of chosenExt) params.append("ext", ext);
      for (const store of storeFilter.stores()) params.append("store", store);
      params.set("limit", String(FILES_PER_REQUEST));
      params.set("offset", String(offset));
      let data;
      try {
        data = await api(`/api/files?${params}`);
      } catch (e) {
        bulkNote.className = "note bad";
        bulkNote.textContent = e.message;
        return;
      }
      for (const file of data.files || []) picked.set(file.path, file.store);
      if (!(data.files || []).length) break;
    }
    bulkNote.textContent = "";
    for (const box of table.node.querySelectorAll("tbody input.pick")) box.checked = true;
    paintBulk();
  }

  // The formats this release added are on the list, so they are one click away
  // rather than something you have to know to type.
  const EXTENSIONS = ["rs", "md", "py", "ts", "js", "go", "pdf", "docx", "epub", "png", "jpg"];
  const extChips = EXTENSIONS.map((ext) =>
    el("button", {
      class: "chip",
      type: "button",
      "aria-pressed": "false",
      text: `.${ext}`,
      onclick: (e) => {
        const on = e.currentTarget.getAttribute("aria-pressed") !== "true";
        e.currentTarget.setAttribute("aria-pressed", String(on));
        if (on) chosenExt.add(ext);
        else chosenExt.delete(ext);
        clearTimeout(load.timer);
        load(true);
      },
    }),
  );

  load();

  const indexedPane = el(
    "div",
    { class: "rows files-pane" },
    el(
      "div",
      { class: "files-filters" },
      el(
        "div",
        { class: "filters" },
        el(
          "div",
          { class: "field wide" },
          icon(ICONS.search, 15),
          labelled("files-filter", "Filter files by path glob", pathInput),
        ),
      ),
      el("div", { class: "filters" }, el("span", { class: "filter-label", text: "Type" }), extChips),
      el("div", { class: "filters" }, el("span", { class: "filter-label", text: "Store" }), storeFilter.node),
    ),
    bulkBar,
    bulkNote,
    holder,
    footnote,
  );
  const treePane = filesTree(storeFilter);
  const decisionsTab = decisionsPane();
  const panes = [
    ["Indexed", indexedPane, null],
    ["Tree", treePane.node, treePane.load],
    ["Decisions", decisionsTab.node, decisionsTab.load],
  ];
  const panel = el("div", { class: "tab-panel" });
  const tabButtons = panes.map(([label], i) =>
    el("button", {
      class: "tab",
      type: "button",
      text: label,
      "data-tab": label,
      "aria-pressed": String(i === 0),
      onclick: () => showPane(i),
    }),
  );
  function showPane(index) {
    tabButtons.forEach((b, i) => b.setAttribute("aria-pressed", String(i === index)));
    fill(panel, panes[index][1]);
    if (panes[index][2]) panes[index][2]();
  }
  showPane(state.filesTab === "decisions" ? 2 : state.filesTab === "tree" ? 1 : 0);
  state.filesTab = "";

  return el(
    "div",
    { class: "view" },
    unreadable,
    pageHead(
      "Files",
      "What is indexed, and which reader parsed it — so “not indexed” and “not discussed” stop looking the same.",
      { pill: summary },
    ),
    el("div", { class: "tabs" }, tabButtons),
    panel,
    announcer,
  );
}

/* The tree view: an editor's explorer over what each store holds.
 *
 * One level at a time, read when a folder is opened, so a tree of a hundred
 * thousand files costs what the open folders hold. Each store is a root; a
 * folder shows how many files it holds, a file its lines and symbols, and
 * what sits on disk but is not indexed is listed greyed with why. The same
 * facts `semlith_files {tree: true}` gives an agent as text. */
const FILE_TYPES = [
  [/\.rs$/, "ft-rust"],
  [/\.(m?js|cjs|jsx)$/, "ft-js"],
  [/\.tsx?$/, "ft-ts"],
  [/\.py$/, "ft-py"],
  [/\.(md|markdown|txt|rst)$/, "ft-md"],
  [/\.(json|ya?ml|toml|lock)$/, "ft-data"],
  [/\.(html?|css|scss|svg)$/, "ft-web"],
  [/\.(png|jpe?g|gif|webp|ico)$/, "ft-image"],
];
const fileType = (name) => (FILE_TYPES.find(([re]) => re.test(name.toLowerCase())) || [null, ""])[1];

function filesTree(storeFilter) {
  const sort = "name";
  const list = el("ul", { role: "tree", "aria-label": "Indexed files, by folder" });
  const box = el("div", { class: "card ftree" }, list);

  const url = (store, dir) => {
    const params = new URLSearchParams({ tree: "1", format: "json", sort });
    if (dir) params.set("dir", dir);
    if (store) params.append("store", store);
    else for (const name of storeFilter.stores()) params.append("store", name);
    return `/api/files?${params}`;
  };

  function row(level, kind, name, meta, extra) {
    const guides = [el("span", { class: "indent lead" })];
    for (let i = 1; i < level; i++) guides.push(el("span", { class: "indent" }));
    const dir = kind === "dir" || kind === "root";
    return el(
      "div",
      {
        class: `ftree-row ${kind === "root" ? "dir root" : kind}${extra?.cls ? ` ${extra.cls}` : ""}`,
        role: "treeitem",
        "aria-level": String(level),
        "aria-expanded": dir ? "false" : null,
        tabindex: "-1",
        title: extra?.title || null,
      },
      guides,
      el("span", { class: "chev" }, dir ? icon(ICONS.chevron, 12) : null),
      el(
        "span",
        { class: `ficon${kind === "file" ? ` ${fileType(name)}` : ""}` },
        icon(dir ? ICONS.folder : ICONS.file, 15),
      ),
      el("span", { class: "fname", text: name }),
      meta ? el("span", { class: "fmeta", text: meta }) : null,
    );
  }

  /* A folder: its row, and its children under it, read the first time it
   * opens. `where` is the store and root the folder belongs to. */
  function folder(where, dirPath, name, meta, level, kind, preload) {
    const head = row(level, kind, name, meta, { title: `${where.root}/${dirPath}`.replace(/\/$/, "") });
    const kids = el("ul", { role: "group", hidden: "" });
    const item = el("li", {}, head, kids);
    let loaded = false;
    const glyph = head.querySelector(".ficon");
    async function open(want) {
      const on = want === undefined ? kids.hidden : want;
      head.setAttribute("aria-expanded", String(on));
      kids.hidden = !on;
      fill(glyph, icon(on ? ICONS.folderOpen : ICONS.folder, 15));
      if (!on || loaded) return;
      loaded = true;
      fill(kids, el("li", { class: "ftree-more", text: "Reading…" }));
      try {
        const data = await api(url(where.store, dirPath));
        const mine = (data.roots || []).find((r) => r.store === where.store && r.root === where.root);
        children(kids, where, dirPath, mine, level + 1);
      } catch (e) {
        loaded = false;
        fill(kids, el("li", { class: "ftree-more", text: e.message }));
      }
    }
    head.addEventListener("click", () => open());
    head.addEventListener("keydown", (e) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        open();
      } else if (e.key === "ArrowRight") open(true);
      else if (e.key === "ArrowLeft") open(false);
    });
    if (preload) {
      loaded = true;
      children(kids, where, dirPath, preload, level + 1);
      head.setAttribute("aria-expanded", "true");
      kids.hidden = false;
      fill(glyph, icon(ICONS.folderOpen, 15));
    }
    return item;
  }

  function children(ul, where, dirPath, entry, level) {
    const items = [];
    const under = (name) => (dirPath ? `${dirPath}/${name}` : name);
    for (const d of entry?.dirs || []) {
      items.push(folder(where, under(d.name), d.name, `${n(d.files)} file${d.files === 1 ? "" : "s"}`, level, "dir"));
    }
    for (const f of entry?.files || []) {
      const meta = f.lines ? `${n(f.lines)} lines · ${n(f.symbols)} symbol${f.symbols === 1 ? "" : "s"}` : `${n(f.chunks)} chunks`;
      items.push(el("li", {}, row(level, "file", f.name, meta, { cls: f.stale ? "stale" : "", title: `${where.root}/${under(f.name)}` })));
    }
    for (const o of entry?.not_indexed || []) {
      items.push(el("li", {}, row(level, "file", o.name, `not indexed — ${o.why}`, { cls: "off", title: `${where.root}/${under(o.name)}` })));
    }
    if (entry?.more) items.push(el("li", { class: "ftree-more", text: `+ ${n(entry.more)} more in this folder` }));
    if (!items.length) items.push(el("li", { class: "ftree-more", text: "Empty." }));
    fill(ul, items);
  }

  async function load() {
    fill(list, el("li", { class: "ftree-more", text: "Reading…" }));
    let data;
    try {
      data = await api(url(null, ""));
    } catch (e) {
      fill(list, el("li", { class: "ftree-more", text: e.message }));
      return;
    }
    const roots = data.roots || [];
    if (!roots.length) {
      fill(list, el("li", { class: "ftree-more", text: "Nothing indexed yet." }));
      return;
    }
    fill(
      list,
      roots.map((r) => {
        const where = { store: r.store, root: r.root };
        const leaf = r.root.split("/").filter(Boolean).pop() || r.root;
        const label = leaf === r.store ? r.store : `${r.store} · ${leaf}`;
        return folder(where, "", label, null, 1, "root", r);
      }),
    );
  }

  // Folders first, then files, by name, as an editor's explorer orders them.
  const node = box;
  return { node, load };
}

/* Decisions already made about files the scan held back (2.5): files a
 * person accepted, redacted or as-is, files they refused, and files the scan
 * let through because every match is a declared test dummy. Each one can be
 * undone here, one file at a time. New decisions are made on the Index page
 * after a scan; what was not indexed and needs nothing is said there too. */
const CLASS_LABELS = {
  content: "secret-shaped value",
  credential: "credential file",
  policy: "policy limit",
  unindexable: "not indexable",
  excluded: "your exclusions",
  dummy: "test dummies",
};

function decisionsPane() {
  const node = el("div", { class: "rows files-pane decisions" });

  function refuseDummy(store, row) {
    const reviewed = el("input", { type: "checkbox", id: "reviewed-file" });
    ask({
      title: "Refuse this file?",
      body: "Every match in it is a declared test dummy, so it was indexed. Refusing takes it out of this store until you revoke the decision.",
      lead: dialogPath(row.path, store),
      extra: row.matches && row.matches.length ? findingsList(row.matches) : null,
      tail: el("label", { class: "reviewed", for: "reviewed-file" }, reviewed, el("span", { text: "I have reviewed this file" })),
      confirm: "Refuse this file",
      tone: "bad",
      wide: true,
      run: async () => {
        if (!reviewed.checked) throw new Error("Tick “I have reviewed this file” first.");
        await post("/api/refused/accept", { store, path: row.path, mode: "refused", reviewed: true });
        await load();
      },
    });
  }

  function revoke(store, row) {
    ask({
      title: "Revoke this decision?",
      body:
        row.accepted === "refused"
          ? "The file is indexed again at the next run, as a test-dummy file."
          : "The file leaves the store and is refused again with today's reasons; the next scan offers it for a decision.",
      lead: dialogPath(row.path, store),
      confirm: "Revoke",
      tone: "bad",
      wide: true,
      run: async () => {
        await post("/api/refused/revoke", { store, path: row.path });
        await load();
      },
    });
  }

  const decisionLabel = (row) =>
    row.accepted === "refused"
      ? "refused by you"
      : row.accepted === "redacted"
        ? "accepted, redacted"
        : row.accepted
          ? "accepted as-is"
          : "let through";

  function table(store, rows) {
    return dataTable({
      className: "w-decisions",
      caption: `Decisions about files in ${store}`,
      rows,
      columns: [
        {
          key: "path",
          label: "File",
          value: (r) => rootRel(r.path, store),
          render: (r) => pathCell(rootRel(r.path, store), "path"),
        },
        {
          key: "decision",
          label: "Decision",
          value: decisionLabel,
          render: (r) => el("span", { class: r.accepted && r.accepted !== "refused" ? "pill good" : "pill", text: decisionLabel(r) }),
        },
        { key: "rule", label: "Why it was held back", sortable: false, className: "narrow-drop", render: (r) => lineCell(r.class === "dummy" ? "every match is a declared test dummy" : r.rule) },
        {
          key: "confidence",
          label: "Likely real",
          className: "num",
          value: (r) => (r.confidence == null ? -1 : r.confidence),
          render: (r) => (r.confidence == null ? "—" : `${r.confidence} %`),
        },
        {
          key: "action",
          label: "",
          sortable: false,
          className: "decide-col",
          render: (r) =>
            r.accepted
              ? el("button", { class: "button secondary small", type: "button", text: "Revoke", onclick: () => revoke(store, r) })
              : el("button", { class: "button secondary small", type: "button", text: "Refuse instead", onclick: () => refuseDummy(store, r) }),
        },
      ],
    }).node;
  }

  async function load() {
    fill(node, el("p", { class: "subtitle", text: "Reading…" }));
    let data;
    try {
      data = await api("/api/refused");
    } catch (e) {
      fill(node, error(e.message));
      return;
    }
    const blocks = [
      el("p", {
        class: "subtitle",
        text: "Files you accepted or refused, and files let through because every match is a test dummy. Undo any one of them here; new decisions are made on the Index page after a scan.",
      }),
    ];
    for (const store of data.stores || []) {
      const rows = (store.rows || []).filter((r) => r.accepted || r.class === "dummy");
      if (!rows.length) continue;
      blocks.push(el("h2", { class: "section-title", text: store.store }));
      blocks.push(table(store.store, rows));
    }
    if (blocks.length === 1) {
      blocks.push(el("div", { class: "card pad" }, empty("No decisions yet. When a scan holds a file back, the Index page asks about it.")));
    }
    fill(node, ...blocks);
  }
  return { node, load };
}

/* What the scanner matched in one file, for a decision dialog: each value
 * masked, what kind it looked like and where, how likely it is to be real,
 * and the signals behind that number on a line of their own. One font per
 * role, so the list reads as a table and not as a sentence. */
function findingsList(matches) {
  const signals = (m) =>
    (m.signals || []).map((s) => `${s.name} ${s.effect === "up" ? "↑" : "↓"} ${s.detail}`).join(" · ");
  return el(
    "ul",
    { class: "findings" },
    (matches || []).map((m) =>
      el(
        "li",
        {},
        el(
          "div",
          { class: "finding-head" },
          el("code", { class: "finding-value", text: m.masked }),
          el("span", { class: "finding-kind", text: `${m.kind} · line ${m.line}` }),
          el("span", { class: "spacer" }),
          el("strong", { class: "finding-conf", text: `${m.confidence} % likely real` }),
        ),
        signals(m) ? el("div", { class: "finding-signals", text: signals(m) }) : null,
      ),
    ),
  );
}

/** The file a dialog is about: its path under the store's root, whole and
 * wrapping, with the machine's absolute path on the tooltip. */
function dialogPath(path, store) {
  return el("code", { class: "dialog-path", title: path, text: rootRel(path, store) });
}

/* One file's decision from the scan panel, with the mode its button chose:
 * the file, what was found in it, and the tick that says it was read. */
function reviewOne(store, item, mode, done) {
  const reviewed = el("input", { type: "checkbox", id: "reviewed-inline" });
  ask({
    title: mode === "redacted" ? "Accept with redaction?" : "Accept as-is?",
    body:
      mode === "redacted"
        ? "Each detected value is replaced by [REDACTED:…] before anything is stored. Redaction covers only what the scanner detected."
        : item.class === "content"
          ? "The file's full text is indexed, values included."
          : `${item.rule}. Accepting indexes it anyway.`,
    lead: dialogPath(item.path, store),
    extra: item.matches && item.matches.length ? findingsList(item.matches) : null,
    tail: el(
      "label",
      { class: "reviewed", for: "reviewed-inline" },
      reviewed,
      el("span", { text: "I have reviewed this file" }),
    ),
    confirm: "Accept this file",
    wide: true,
    run: async () => {
      if (!reviewed.checked) throw new Error("Tick “I have reviewed this file” first.");
      await post("/api/refused/accept", { store, path: item.path, mode, reviewed: true });
      done();
    },
  });
}

// ------------------------------------------------------- read and pattern

/* Which store a question is asked of. One choice rather than a row of
 * independent toggles: "All stores" is a state a reader can name, where three
 * chips half-lit is a filter they have to reconstruct from the lighting. */
function storePicker(initial, onChange) {
  let picked = initial || "";
  const row = el("div", { class: "store-chips" });
  const paint = () => {
    for (const chip of row.children) {
      chip.setAttribute("aria-pressed", String(chip.getAttribute("data-store") === picked));
    }
  };
  const add = (name, label) =>
    row.append(
      el("button", {
        class: "chip",
        type: "button",
        "data-store": name,
        text: label,
        onclick: () => {
          picked = name;
          paint();
          onChange();
        },
      }),
    );
  add("", "All stores");
  for (const store of liveStores()) add(store.name, store.name);
  paint();
  return { node: row, stores: () => (picked ? [picked] : []) };
}

/* Whether the file still looks the way it did when it was indexed — the dot
 * and the word together. The dot alone was a colour with the meaning in a
 * tooltip, which is no meaning at all on a phone or in a screenshot. */
function freshMark(fresh) {
  const ok = fresh !== false;
  return el(
    "span",
    {
      class: "fresh",
      title: ok ? "indexed from the bytes on disk" : "file changed since it was indexed",
    },
    el("span", { class: `fresh-dot ${ok ? "is-fresh" : "is-stale"}` }),
    el("span", { class: "word", text: ok ? "fresh" : "stale" }),
  );
}

/* Code with a line-number gutter, numbered from the span's own first line —
 * the same coordinates `semlith read` and every error message print, so a
 * number read here can be typed back in without arithmetic. */
function codeGutter(text, startLine) {
  const from = Number(startLine) || 1;
  return el(
    "div",
    { class: "gutter-code" },
    String(text == null ? "" : text)
      .split("\n")
      .map((line, i) =>
        el(
          "div",
          { class: "code-line" },
          el("span", { class: "ln", text: String(from + i) }),
          el("span", { class: "lt", text: line }),
        ),
      ),
  );
}

/** One span, as `/api/read` returns it. `badges` go in its header, before the
 * freshness mark: the Brief view says which lists found each span there. */
function spanCard(span, badges) {
  const named = span.symbol ? `${span.symbol_kind || ""} ${span.symbol}`.trim() : "";
  return el(
    "div",
    { class: "span-card" },
    el(
      "div",
      { class: "span-head" },
      el("span", { class: "path", text: span.path }),
      el("span", { class: "lines", text: `${span.start_line}-${span.end_line}` }),
      span.store ? el("span", { class: "from", text: span.store }) : null,
      el("span", { class: "spacer" }),
      badges || null,
      freshMark(span.fresh),
    ),
    named ? el("div", { class: "sig", text: named }) : null,
    codeGutter(span.text, span.start_line),
  );
}
// ---------------------------------------------------------------- search

/* Which of the ranked lists found a hit. The word rather than an initial: a
 * hit the graph alone reached is a neighbour of a match rather than a match,
 * and `g` said that only to a reader who hovered it. */
const LIST_LABELS = {
  definition: "definition — this chunk defines the name that was typed",
  vector: "vector — the embedding matched",
  keyword: "full text — the terms matched",
  graph: "graph — reached from a neighbouring symbol",
  image: "image — the picture matched the words",
};

function fusionBadges(lists) {
  if (!lists || !lists.length) return null;
  return el(
    "span",
    { class: "badges" },
    lists.map((list) =>
      el("span", { class: `badge ${list}`, title: LIST_LABELS[list] || list, text: list }),
    ),
  );
}

/* The four confidence values an edge or a graph-found hit can carry, and what
 * each one actually claims. Never rendered without one: a hop with no badge is
 * a hop a reader will assume was verified. */
const CONFIDENCE = {
  extracted: ["extracted", "the import names the target"],
  resolved: ["resolved", "name and module hint agree on one definition"],
  inferred: ["inferred", "matched by bare name"],
  ambiguous: ["ambiguous", "several definitions, none chosen"],
};

function confidenceBadge(value) {
  if (!value) return null;
  const [label, why] = CONFIDENCE[value] || [value, ""];
  return el("span", { class: `conf ${value}`, title: why, text: label });
}

/* What a graph-reached hit's provenance means, as a sentence rather than as a
 * badge nobody can expand. A reader who does not know what `inferred` claims
 * reads every row as verified. */
const PROVENANCE_NOTE = {
  extracted: "Found through the graph: the source names the target, so nothing had to be ranked.",
  resolved: "Found through the graph: the name and its module hint agree on one definition.",
  inferred: "Found through the graph by bare name. Corroborate before you rely on it.",
  ambiguous:
    "Found through the graph, but several definitions carry this name and none was chosen.",
};

/* The one line of a chunk worth showing in a locator row: the line with most
 * of the query's words in it, or the first line with anything on it. */
function bestLine(text, query) {
  const terms = (query || "")
    .split(/[^A-Za-z0-9_]+/)
    .filter((t) => t.length > 2)
    .map((t) => t.toLowerCase());
  let best = null;
  let bestScore = -1;
  for (const raw of (text || "").split("\n")) {
    const line = raw.trim();
    if (!line) continue;
    const lowered = line.toLowerCase();
    const score = terms.filter((t) => lowered.includes(t)).length;
    if (score > bestScore) {
      bestScore = score;
      best = line;
    }
  }
  if (!best) return "";
  // Capped as the tool caps it, rather than left to the stylesheet's ellipsis:
  // a row is one line of context, and a minified file would otherwise put a
  // whole line of it into the page for the browser to hide.
  return best.length > 120 ? `${best.slice(0, 120)}…` : best;
}

/* What one locator row costs, counted the way `mcp::locate` counts it: the
 * text the row actually renders, at four characters to a token. */
function rowCost(hit, query) {
  const named = hit.symbol ? `${hit.symbol_kind || ""} ${hit.symbol}`.trim() : "";
  const marks = (hit.lists || []).join("+") + (hit.fresh === false ? " stale" : "");
  const row = `  ${hit.start_line}-${hit.end_line} ${named} ${marks}\n    ${bestLine(
    hit.text,
    query,
  )}\n`;
  return Math.ceil(row.length / 4);
}

/** The likeliest symbol name in a hit, for the link into the graph. */
function symbolIn(hit) {
  const match = hit.text.match(
    /(?:fn|function|def|func|class|struct|interface|type)\s+([A-Za-z_][A-Za-z0-9_]*)/,
  );
  return match ? match[1] : "";
}

async function searchView() {
  await refreshStores();

  const results = el("div", { class: "results locate-list" });
  const bodyCard = el("div", { class: "span-card" });
  const aroundName = el("span", { class: "sym" });
  const footer = el("div", { class: "locate-footer" });
  const meta = el("span", { class: "search-meta" });
  /* The shape the ranker read and what it did about it, from the answer's own
   * fields. Re-deriving the rule here would be a second classifier, and only
   * one of the two ranked the hits underneath it. */
  const shapeHint = el("div", { class: "shape-hint", hidden: true });

  /* The second ring, drawn rather than linked. One canvas for the whole visit
   * to the page: a fresh one per opened hit would be a fresh animation loop
   * per click. */
  const ego = graphCanvas({
    /* The same two handlers the Graph page passes, so the panel behaves like
     * the page it is a window onto rather than like a picture of it: a node
     * lifts on hover with a card naming it, and a click selects it and its
     * neighbours. Without these the hint underneath — "drag a node · click to
     * select" — described something that did not happen. */
    onHover: (node, x, y) => {
      if (!node) return tip.hide("ego");
      tip.atPoint(
        x,
        y,
        tipCard(
          node.name,
          "blue",
          [
            node.kind ? ["kind", node.kind] : null,
            node.path ? ["file", `${shortPath(node.path)}:${node.start_line}-${node.end_line}`] : null,
          ].filter(Boolean),
        ),
        "ego",
      );
    },
    onPick: (node) => {
      aroundName.textContent = node ? node.name : "—";
    },
  });
  const stage = el(
    "div",
    { class: "ego-panel", hidden: true },
    ego.node,
    el("span", { class: "ego-hint", text: "drag a node · click to select" }),
  );
  /* Both hidden until a row is open. The panel is tall enough to be a graph
   * rather than a thumbnail, which makes it a large empty box on a page nobody
   * has searched yet — and "Around" with nothing after it is a heading for
   * something that is not there. */
  const aroundHead = el(
    "div",
    { class: "around-head", hidden: true },
    el("span", { class: "title" }, "Around ", aroundName),
    el("span", { class: "meta", text: "graph expansion" }),
  );
  const showRing = (on) => {
    stage.hidden = !on;
    aroundHead.hidden = !on;
  };

  // The dials, in the design's order. Each is the same control an agent sets
  // on the tool, shown here so a person can see what the agent is holding.
  let k = 8;
  const kValue = el("span", { class: "dial-value", text: "k = 8" });
  const bump = (delta) => {
    const next = Math.min(50, Math.max(1, k + delta));
    if (next === k) return;
    k = next;
    kValue.textContent = `k = ${k}`;
    run();
  };
  const kDial = el(
    "div",
    { class: "dial" },
    kValue,
    el("button", { class: "step", type: "button", "aria-label": "fewer hits", title: "fewer hits", text: "–", onclick: () => bump(-1) }),
    el("button", { class: "step", type: "button", "aria-label": "more hits", title: "more hits", text: "+", onclick: () => bump(1) }),
  );

  /* Which side of the corpus to lean towards. Live from 0.16.0: the query text
   * says how a question was written, and this says what the asker is after,
   * which the text cannot. */
  let prefer = "any";
  const preferPills = ["code", "docs", "any"].map((name) =>
    el("button", {
      class: "seg",
      type: "button",
      "aria-pressed": String(name === prefer),
      text: name,
      onclick: () => {
        if (prefer === name) return;
        prefer = name;
        for (const pill of preferPills) {
          pill.setAttribute("aria-pressed", String(pill.textContent === prefer));
        }
        run();
      },
    }),
  );
  const preferDial = el(
    "div",
    { class: "dial" },
    el("span", { class: "dial-label", text: "Prefer" }),
    el("span", { class: "segs" }, preferPills),
  );

  /* Two answers to one question, from the same store and the same ranking.
   *
   * `results` is the two-stage list this page has always been: locate, then
   * open a row and read it. `brief` is what an agent gets from one call --
   * the same spans, the text of the top ones, and the one-hop callers and
   * callees of the symbols they sit inside, all of it fitted to the budget
   * dial beside it. Free, because every primitive under it is free.
   *
   * A view rather than a second page: it is the same question, and making a
   * person retype it somewhere else to see the other shape of the answer is
   * how two pages end up disagreeing about what the store says. */
  let view = "results";
  const viewPills = ["results", "brief", "exact"].map((name) =>
    el("button", {
      class: "seg",
      type: "button",
      "aria-pressed": String(name === view),
      text: name,
      onclick: () => {
        if (view === name) return;
        view = name;
        for (const pill of viewPills) {
          pill.setAttribute("aria-pressed", String(pill.textContent === view));
        }
        run();
      },
    }),
  );
  const viewDial = el(
    "div",
    { class: "dial" },
    el("span", { class: "dial-label", text: "View" }),
    el("span", { class: "segs" }, viewPills),
  );

  /* Re-run as the filter is typed, like every other control on this page.
   *
   * `change` alone fires on blur, so a reader who typed a glob and looked at
   * the rows was looking at results from before the glob — with the filter
   * field showing the new value and nothing saying the two disagreed. */
  let filterTimer = 0;
  const rerun = () => {
    clearTimeout(filterTimer);
    filterTimer = setTimeout(() => run(), 250);
  };
  const langField = el("input", {
    class: "bare",
    size: "8",
    placeholder: "any",
    oninput: rerun,
    onchange: () => run(),
  });
  const pathField = el("input", {
    class: "bare",
    size: "8",
    placeholder: "any",
    oninput: rerun,
    onchange: () => run(),
  });
  const langDial = el(
    "div",
    { class: "dial" },
    el("span", { class: "dial-prefix", text: "lang:" }),
    labelled("search-lang", "Only this language", langField),
  );
  const pathDial = el(
    "div",
    { class: "dial" },
    el("span", { class: "dial-prefix", text: "path:" }),
    labelled("search-path", "Only paths matching this glob", pathField),
  );

  const budgetField = el("input", {
    class: "bare",
    size: "5",
    value: "1500",
    inputmode: "numeric",
    oninput: rerun,
    onchange: () => run(),
  });
  const budgetDial = el(
    "div",
    { class: "dial" },
    el("span", { class: "dial-label", text: "Budget" }),
    labelled("search-budget", "Budget in tokens", budgetField),
    el("span", { class: "unit", text: "tok" }),
  );
  const budget = () => Math.max(200, Number(String(budgetField.value).replace(/[^0-9]/g, "")) || 1500);

  // Restored rather than reset: the question and the store filter survive a
  // trip to another page, because coming back to an empty box means typing it
  // again.
  const picker = storePicker(state.search.stores[0] || "", () => run());
  let generation = 0;

  const input = el("input", {
    type: "search",
    // The same words as the launcher in the top bar. Two phrasings for one
    // destination reads as two destinations.
    placeholder: "Ask the index a question",
    oninput: () => {
      // The hint is about the query in the box. An empty box has no shape.
      if (!input.value.trim()) shapeHint.hidden = true;
    },
    onkeydown: (e) => {
      if (e.key === "Enter") run();
    },
  });

  /* The second stage's column, which the Brief view has no use for: a brief
   * *is* the opened row, so "pick a row, the span opens here" would be an
   * instruction for something that has already happened. Hidden by a class on
   * the container rather than by emptying the column, so the locate side takes
   * the whole width instead of leaving a gap where the panel was. */
  const bodyCol = el("div", { class: "body-col" }, bodyCard, aroundHead, stage);
  const twoStage = el(
    "div",
    { class: "two-stage" },
    el("div", { class: "locate-col" }, results, footer),
    bodyCol,
  );

  const nothing = () =>
    empty("Type a phrase or an identifier. Both halves of the search run either way.");

  async function run() {
    const query = input.value.trim();
    state.search = { query, stores: picker.stores() };
    if (!query) {
      shapeHint.hidden = true;
      fill(results, nothing());
      showBody(null, "");
      fill(footer);
      meta.textContent = "";
      return;
    }

    twoStage.classList.remove("one-stage");
    if (view === "brief") {
      twoStage.classList.add("one-stage");
      return runBrief(query);
    }
    if (view === "exact") return runExact(query);

    const mine = ++generation;
    const params = new URLSearchParams({ query, k: String(k), prefer });
    for (const store of picker.stores()) params.append("store", store);
    const lang = langField.value.trim();
    const path = pathField.value.trim();
    if (lang) params.append("lang", lang);
    if (path) params.append("path", path);

    meta.textContent = "searching…";
    /* The first search after the daemon starts loads the embedding model,
     * which is five seconds on a cold cache — and until this it was five
     * seconds of "searching…" that looked like a search that had hung. Only
     * shown once a search has taken longer than a warm one ever does. */
    const slow = setTimeout(() => {
      if (mine === generation) {
        meta.textContent = "loading the embedding model — the first search after the daemon starts pays for it once";
      }
    }, 1200);
    let data;
    try {
      data = await api(`/api/search?${params}`);
    } catch (e) {
      clearTimeout(slow);
      if (mine !== generation) return;
      fill(results, error(e.message));
      showBody(null, "");
      fill(footer);
      meta.textContent = "";
      return;
    }
    clearTimeout(slow);
    if (mine !== generation) return;

    if (data.shape_label) {
      fill(
        shapeHint,
        el("span", { class: "dot" }),
        el("span", { text: `${data.shape_label} · ${data.weighting}` }),
      );
      shapeHint.hidden = false;
    }

    // Hits and how long, which is what the question was. The corpus size is a
    // fact about the store, and the Stores page is where it is asked.
    meta.textContent = data.hits.length
      ? `${data.hits.length} hit${data.hits.length === 1 ? "" : "s"} · ${(data.micros / 1000).toFixed(1)} ms`
      : "";

    if (!data.hits.length) {
      showBody(null, "");
      fill(footer);
      fill(
        results,
        empty(
          "No chunk in the selected stores matches that. If a store chip is on it is the filter, not the corpus — clear it and ask again.",
        ),
      );
      return;
    }

    // Grouped by file, in the order the ranking put the files in: eight hits
    // in one file are one file to open, and the best hit's file is the first
    // thing read.
    const groups = [];
    for (const hit of data.hits) {
      const found = groups.find((g) => g.path === hit.path && g.store === hit.store);
      if (found) found.hits.push(hit);
      else groups.push({ path: hit.path, store: hit.store, hits: [hit] });
    }

    // Cut to the budget, lowest-ranked first, exactly as the tool does — and
    // the footer says so. A footer that reported a budget nothing enforced
    // would be a number for decoration; a list that silently dropped rows
    // would be worse.
    const cap = budget();
    let spent = 0;
    const shown = [];
    for (const group of groups) {
      // What the *rows* cost, not what the chunks behind them cost. A locate
      // row is one line of a chunk however long the chunk is, which is the
      // whole point of the format — measuring the chunk here would make the
      // page cut at a tenth of the budget the tool cuts at, for the same
      // number in the same box.
      const cost = group.hits.reduce((total, hit) => total + rowCost(hit, query), 4);
      // The first group always shows, whatever it costs: a budget that
      // returned nothing would turn a search into a silent failure.
      if (shown.length && spent + cost > cap) break;
      spent += cost;
      shown.push(group);
    }
    const kept = shown.reduce((total, group) => total + group.hits.length, 0);

    fill(
      results,
      shown.map((group) => {
        const spans = group.hits.map((hit) => `${hit.start_line}-${hit.end_line}`);
        return el(
          "div",
          { class: "locate-group" },
          el(
            "div",
            { class: "locate-file" },
            el("span", { class: "file", "data-tip": group.path, text: shortPath(group.path) }),
            group.store ? el("span", { class: "from", text: group.store }) : null,
            el("span", { class: "spacer" }),
            el("span", {
              class: "span-summary",
              text: spans.length > 1 ? `${spans.length} spans · ${spans.join(", ")}` : spans[0],
            }),
          ),
          group.hits.map((hit) => locateRow(hit, query)),
        );
      }),
    );
    fill(
      footer,
      el("span", {
        text: `${kept} of ${data.hits.length} shown · ${n(spent)} tokens · budget ${n(cap)}`,
      }),
      kept < data.hits.length
        ? el("span", {
            class: "truncated",
            text: `truncated: ${kept} of ${data.hits.length}`,
          })
        : null,
    );
    // The body panel starts on the best hit rather than empty: the first
    // question anyone has about a result list is what the top one says.
    const first = results.querySelector(".locate-row");
    if (first) first.setAttribute("aria-pressed", "true");
    showBody(shown[0].hits[0], query);
  }

  /* One locator line: where it is, what it is called, how it was found,
   * whether the file has moved under it, and one line of it. Both lines sit
   * inside the one control, so the excerpt is part of the target rather than
   * a strip of dead pixels under it. */
  function locateRow(hit, query) {
    const line = bestLine(hit.text, query);
    const row = el(
      "button",
      {
        class: "locate-row",
        type: "button",
        "aria-pressed": "false",
        onclick: () => {
          for (const other of results.querySelectorAll(".locate-row")) {
            other.setAttribute("aria-pressed", "false");
          }
          row.setAttribute("aria-pressed", "true");
          showBody(hit, query);
        },
      },
      el(
        "span",
        { class: "row-top" },
        el("span", {
          class: "lines",
          text: hit.image
            ? `${hit.image.width}×${hit.image.height} px`
            : `${hit.start_line}-${hit.end_line}`,
        }),
        // A hit with no symbol is prose, said rather than left as a gap the
        // reader has to interpret. An image is neither, and its badge and its
        // pixel dimensions have already said so.
        hit.image
          ? null
          : el("span", {
              class: "sym",
              text: hit.symbol ? `${hit.symbol_kind || ""} ${hit.symbol}`.trim() : "prose",
            }),
        fusionBadges(hit.lists),
        el("span", { class: "spacer" }),
        freshMark(hit.fresh),
        // Only for a hit the graph reached: a vector match has no provenance
        // to state beyond the badge it already carries.
        confidenceBadge(hit.provenance),
      ),
      line ? el("span", { class: "locate-excerpt", text: line }) : null,
    );
    return row;
  }

  /* The second stage: the span itself, once a row has been chosen.
   *
   * What the row already carries is painted at once; the signature, the
   * symbol's whole span and the neighbourhood arrive from `/api/read` and
   * `/api/symbol` after it, because none of them is worth a spinner over the
   * text the reader clicked for.
   */
  let openHit = null;
  let openQuery = "";
  let wholeSpan = null;
  let signature = "";
  let whole = false;
  /* How many definitions the second read found. `many` is not a failure — it
   * is the store saying it cannot tell which one this is, and the button says
   * that rather than pretending there is nothing more to open. */
  let definitions = 0;
  let bodyGeneration = 0;

  /* The Brief view: one call, rendered as it comes back.
   *
   * Nothing is re-derived here. The labels on each part -- which ranked list
   * found a span, and that an edge came from the graph -- are the tool's own,
   * because a page that decided for itself which list found something would be
   * a second classifier disagreeing with the one that ranked the answer. */
  /* `semlith_search {exact: true}`: every indexed line matching the query, as
   * grep -E finds it, grouped by file, each row naming the definition it sits
   * in. Opening a row reads that definition whole, which is what a grep user
   * opens the file for. */
  async function runExact(query) {
    const mine = ++generation;
    const params = new URLSearchParams({ query, exact: "1" });
    for (const store of picker.stores()) params.append("store", store);
    const lang = langField.value.trim();
    const path = pathField.value.trim();
    if (lang) params.append("lang", lang);
    if (path) params.append("path", path);
    shapeHint.hidden = true;
    meta.textContent = "searching…";
    let data;
    try {
      data = await api(`/api/search?${params}`);
    } catch (e) {
      if (mine !== generation) return;
      fill(results, error(e.message));
      showBody(null, "");
      fill(footer);
      meta.textContent = "";
      return;
    }
    if (mine !== generation) return;
    const matches = data.matches || [];
    meta.textContent = `${matches.length} line${matches.length === 1 ? "" : "s"} · ${n(data.files || 0)} files searched`;
    if (!matches.length) {
      showBody(null, "");
      fill(footer);
      fill(results, empty("No indexed line matches that. A query that is not a valid regular expression is searched as literal text."));
      return;
    }
    const groups = [];
    for (const m of matches) {
      const found = groups.find((g) => g.path === m.path && g.store === m.store);
      if (found) found.rows.push(m);
      else groups.push({ path: m.path, store: m.store, rows: [m] });
    }
    fill(
      results,
      groups.map((group) =>
        el(
          "div",
          { class: "locate-group" },
          el(
            "div",
            { class: "locate-file" },
            el("span", { class: "file", "data-tip": group.path, text: shortPath(group.path) }),
            group.store ? el("span", { class: "from", text: group.store }) : null,
            el("span", { class: "spacer" }),
            el("span", { class: "span-summary", text: `${group.rows.length} line${group.rows.length === 1 ? "" : "s"}` }),
          ),
          group.rows.map((m) => {
            const hit = {
              path: m.path,
              store: m.store,
              start_line: m.start_line,
              end_line: m.end_line,
              text: m.text,
              symbol: m.capture || null,
            };
            const row = el(
              "button",
              {
                class: "locate-row",
                type: "button",
                "aria-pressed": "false",
                onclick: () => {
                  for (const other of results.querySelectorAll(".locate-row")) {
                    other.setAttribute("aria-pressed", "false");
                  }
                  row.setAttribute("aria-pressed", "true");
                  showBody(hit, query);
                },
              },
              el(
                "span",
                { class: "row-top" },
                el("span", { class: "lines", text: String(m.start_line) }),
                el("span", { class: "sym", text: m.capture || "top level" }),
              ),
              el("span", { class: "locate-excerpt", text: m.text }),
            );
            return row;
          }),
        ),
      ),
    );
    fill(
      footer,
      el("span", { text: `${matches.length} lines · ${groups.length} files` }),
      data.truncated ? el("span", { class: "truncated", text: `truncated at ${matches.length}` }) : null,
    );
    const first = results.querySelector(".locate-row");
    if (first) first.setAttribute("aria-pressed", "true");
    showBody(groups[0].rows.length ? { ...groups[0].rows[0], symbol: groups[0].rows[0].capture || null } : null, query);
  }

  async function runBrief(question) {
    const mine = ++generation;
    const params = new URLSearchParams({ question, budget: String(budget()), prefer });
    for (const store of picker.stores()) params.append("store", store);
    const lang = langField.value.trim();
    const path = pathField.value.trim();
    if (lang) params.append("lang", lang);
    if (path) params.append("path", path);

    showBody(null, "");
    shapeHint.hidden = true;
    meta.textContent = "assembling…";
    const slow = setTimeout(() => {
      if (mine === generation) {
        meta.textContent =
          "loading the embedding model — the first search after the daemon starts pays for it once";
      }
    }, 1200);
    let data;
    try {
      data = await api(`/api/brief?${params}`);
    } catch (e) {
      clearTimeout(slow);
      if (mine !== generation) return;
      fill(results, error(e.message));
      fill(footer);
      meta.textContent = "";
      return;
    }
    clearTimeout(slow);
    if (mine !== generation) return;

    const brief = data.brief || {};
    const spans = brief.spans || [];
    meta.textContent = spans.length ? `one call · ${(data.micros / 1000).toFixed(1)} ms` : "";
    if (!spans.length) {
      fill(footer);
      fill(
        results,
        empty(
          "No chunk in the selected stores matches that. If a store chip is on it is the filter, not the corpus — clear it and ask again.",
        ),
      );
      return;
    }

    /* A span with its text is drawn the way the results view draws a hit:
     * the same card, path header and numbered lines, so the two views of one
     * question look like one product. A span the budget left without text is
     * a compact row that says so, not an empty card. */
    const rows = [];
    for (const span of spans) {
      rows.push(
        span.text
          ? spanCard(span, fusionBadges(span.lists))
          : el(
              "div",
              { class: "brief-head brief-row" },
              el("span", { class: "path", text: `${shortPath(span.path)}:${span.start_line}-${span.end_line}` }),
              span.symbol ? el("span", { class: "sym", text: span.symbol }) : null,
              el("span", {
                class: "brief-dropped",
                text: span.over_budget ? "text left out for the budget" : "text: top span only",
              }),
              el("span", { class: "brief-lists" }, fusionBadges(span.lists)),
            ),
      );
    }
    for (const symbol of brief.symbols || []) {
      const edges = [];
      for (const end of symbol.callers || []) {
        edges.push(
          el(
            "div",
            { class: "brief-edge" },
            el("span", { class: "k", text: "called by" }),
            el("span", { class: "v", text: `${end.name} · ${shortPath(end.path)}:${end.start_line}` }),
          ),
        );
      }
      for (const end of symbol.callees || []) {
        edges.push(
          el(
            "div",
            { class: "brief-edge" },
            el("span", { class: "k", text: "calls" }),
            el("span", { class: "v", text: `${end.name} · ${shortPath(end.path)}:${end.start_line}` }),
          ),
        );
      }
      if (symbol.hidden) {
        edges.push(el("div", { class: "brief-dropped", text: `and ${symbol.hidden} more edges` }));
      }
      rows.push(
        el(
          "div",
          { class: "brief-symbol" },
          el(
            "div",
            { class: "brief-head" },
            el("span", { class: "sym", text: symbol.name }),
            el("span", { class: "brief-lists" }, el("span", { class: "tag", text: symbol.found_by })),
          ),
          ...edges,
        ),
      );
    }
    fill(results, el("div", { class: "brief-view" }, ...rows));

    /* What it cost and what it dropped, in the tool's own numbers. A budget
     * nothing reported would be a number for decoration. */
    const cut = brief.cut || {};
    const dropped = [];
    if (cut.spans) dropped.push(`${cut.spans} spans not located`);
    if (cut.span_text) dropped.push(`${cut.span_text} left without text`);
    if (cut.symbols) dropped.push(`${cut.symbols} symbols' edges`);
    const fact = (label, value) =>
      el("span", { class: "brief-fact" }, el("span", { class: "k", text: label }), el("span", { class: "v", text: value }));
    fill(
      footer,
      el(
        "div",
        { class: "brief-summary" },
        fact("tokens", `${n(brief.tokens)} of ${n(brief.budget)}`),
        fact("spans", n(spans.length)),
        fact("counted with", brief.counted_with || "—"),
        fact("dropped", dropped.length ? dropped.join(", ") : "nothing"),
      ),
    );
  }

  function showBody(hit, query) {
    openHit = hit;
    openQuery = query;
    wholeSpan = null;
    signature = "";
    whole = false;
    definitions = 0;
    const mine = ++bodyGeneration;
    paintBody();
    if (!hit) {
      ego.draw({ nodes: [], edges: [] }, null);
      aroundName.textContent = "—";
      return;
    }

    const target = hit.symbol || `${hit.path}:${hit.start_line}`;
    const params = new URLSearchParams({ target });
    if (hit.store) params.append("store", hit.store);
    api(`/api/read?${params}`)
      .then((data) => {
        if (mine !== bodyGeneration) return;
        if (!data.span) {
          definitions = (data.definitions || []).length;
          if (definitions) paintBody();
          return;
        }
        wholeSpan = data.span;
        // The signature is the first line of the definition, which is where
        // every language this indexes puts it.
        if (hit.symbol) {
          signature = (data.span.text || "")
            .split("\n")
            .map((l) => l.trim())
            .find((l) => l.length > 0) || "";
        }
        paintBody();
      })
      .catch(() => {
        /* The row's own text is already on screen; a failed second read is
         * not worth replacing it with an error. */
      });

    drawAround(hit, mine);
  }

  function paintBody() {
    const hit = openHit;
    if (!hit) {
      fill(
        bodyCard,
        empty("Pick a row. The span opens here, with what it is called and how it was found."),
      );
      showRing(false);
      return;
    }
    const showing = whole && wholeSpan ? wholeSpan : hit;
    const lines = `${showing.start_line}-${showing.end_line}`;
    // Nothing wider to open: a button that redraws the same lines is a button
    // that makes a reader doubt they clicked it.
    const wider =
      wholeSpan &&
      (wholeSpan.start_line < hit.start_line || wholeSpan.end_line > hit.end_line);

    fill(
      bodyCard,
      el(
        "div",
        { class: "span-head" },
        el("span", { class: "path", "data-tip": hit.path, text: shortPath(hit.path) }),
        el("span", { class: "lines", text: lines }),
        el("span", { class: "spacer" }),
        freshMark(hit.fresh),
        confidenceBadge(hit.provenance),
      ),
      hit.provenance && PROVENANCE_NOTE[hit.provenance]
        ? el("p", { class: "prov-note", text: PROVENANCE_NOTE[hit.provenance] })
        : null,
      hit.fresh === false
        ? el("p", {
            class: "note",
            text: "This file has changed since it was indexed. The lines below are what was read then.",
          })
        : null,
      signature ? el("div", { class: "sig", text: signature }) : null,
      hit.image ? imagePreview(hit.path) : codeGutter(showing.text, showing.start_line),
      el(
        "div",
        { class: "span-foot" },
        el("button", {
          class: "button secondary small",
          type: "button",
          disabled: !wider,
          title: wider
            ? null
            : definitions
              ? `${definitions} definitions carry this name, and which one this is was not settled. The Read page lists them.`
              : "this span is already the whole of it",
          text: whole
            ? "Collapse to the matched span"
            : hit.symbol
              ? "Read whole symbol"
              : "Read the whole section",
          onclick: () => {
            whole = !whole;
            paintBody();
          },
        }),
        el("span", {
          class: "body-meta",
          "data-tip": hit.path,
          text: `semlith_read · ${shortPath(hit.path)}:${lines} · second stage`,
        }),
        el("span", { class: "spacer" }),
        el("a", {
          href: "#graph",
          class: "quiet",
          text: "Open in graph",
          onclick: (e) => {
            e.preventDefault();
            state.pendingSymbol = hit.symbol || symbolIn(hit);
            go("graph");
          },
        }),
      ),
    );
  }

  /* The neighbourhood of the open hit, one ring out and then one more, from
   * the evidence block `/api/symbol` already returns. Drawn here rather than
   * linked to, because leaving the page to see what calls this loses the span
   * that raised the question. */
  async function drawAround(hit, mine) {
    const name = hit.symbol || symbolIn(hit);
    aroundName.textContent = name || "—";
    // A hit in prose sits inside no definition, so there is no ring to draw
    // and no heading worth showing over an empty canvas.
    if (!name) {
      showRing(false);
      return ego.draw({ nodes: [], edges: [] }, null);
    }
    showRing(true);
    const params = new URLSearchParams({ name });
    if (hit.store) params.append("store", hit.store);
    let data;
    try {
      data = await api(`/api/symbol?${params}`);
    } catch (_) {
      return;
    }
    if (mine !== bodyGeneration) return;
    ego.draw(egoGraph(name, data), null);
    /* Select the symbol the panel is about, so it arrives drawn in the accent
     * with its neighbours lifted — the state the Graph page puts a focused node
     * in. Before this the centre was one grey box among twenty. */
    ego.pick(name);
  }

  fill(results, nothing());
  paintBody();
  setTimeout(() => {
    ego.start();
    if (state.pendingQuery) {
      input.value = state.pendingQuery;
      state.pendingQuery = "";
      run();
    } else if (state.search.query) {
      input.value = state.search.query;
      run();
    }
    input.focus();
  }, 0);

  return el(
    "div",
    { class: "search-page" },
    el(
      "div",
      { class: "search-band" },
      el("h1", { class: "sr-only", text: "Search" }),
      el(
        "div",
        { class: "search-field" },
        icon(ICONS.search, 18),
        labelled("search-query", "Search the index", input),
      ),
      // Under the box rather than inside it: inside, the count and timing
      // took the input's width and crowded the question being typed.
      meta,
      shapeHint,
      el(
        "div",
        { class: "filters" },
        picker.node,
        el("span", { class: "rule" }),
        kDial,
        preferDial,
        viewDial,
        langDial,
        pathDial,
        budgetDial,
        el("span", { class: "dial-note", text: "what one answer may cost an agent" }),
      ),
    ),
    twoStage,
  );
}

/* The evidence block, as a graph the canvas can draw: the symbol at the
 * centre, its callers and callees around it, and the second ring hung off the
 * first-ring name it was reached through — `via` is the whole of what makes
 * it a second ring rather than another neighbour.
 *
 * A caller or callee row is an `EdgeEnd`, which flattens the symbol it
 * reached into itself: the name is on the row, not under a `symbol` key, and
 * `kind` is the edge's kind rather than the symbol's. */
function egoGraph(name, data) {
  const centre = (data.symbols || [])[0] || {};
  const nodes = [
    {
      name,
      kind: centre.kind || "symbol",
      path: centre.path,
      start_line: centre.start_line,
      end_line: centre.end_line,
    },
  ];
  const index = new Map([[name, 0]]);
  const edges = [];
  /* `where` is the symbol row an edge resolved to, when there was one. The
   * hover card says which file a neighbour lives in, which is most of what
   * anyone wants from it. */
  const add = (label, kind, where) => {
    if (!index.has(label)) {
      index.set(label, nodes.length);
      nodes.push({
        name: label,
        kind,
        path: where && where.path,
        start_line: where && where.start_line,
        end_line: where && where.end_line,
      });
    }
    return index.get(label);
  };
  const link = (from, to, kind, confidence) => {
    if (from !== to) edges.push({ from, to, kind, confidence });
  };
  for (const end of data.callers || []) {
    link(add(end.name, end.kind, end), 0, end.kind, end.confidence);
  }
  for (const end of data.callees || []) {
    link(0, add(end.name, end.kind, end), end.kind, end.confidence);
  }
  for (const hop of data.ego || []) {
    const via = index.get(hop.via);
    if (via === undefined) continue;
    const it = add(hop.name, hop.kind);
    if (hop.direction === "caller") link(it, via, hop.kind, hop.confidence);
    else link(via, it, hop.kind, hop.confidence);
  }
  return { nodes, edges };
}

// ----------------------------------------------------------------- index

/** What the graph covers, per language, for the stores this machine holds.
 *
 * One table, and the same rows `semlith stats` prints. A single store-wide
 * "66 % of call edges resolve" cannot say whether a language is missing edges
 * because its files never parsed or because its calls go somewhere the store
 * does not hold, and those are different problems with different fixes.
 */
/* Graph health, the language mix and chunks by month.
 *
 * All three read the rows `/api/stores?coverage=1` returns, which are the
 * rows `semlith stats` prints — so the page and the terminal cannot disagree
 * about what the graph covers. Nothing here recomputes a count at page load.
 */
/** A span sized as a share of its track, set through the CSSOM.
 *
 * Never a `style` attribute: the portal is served under `style-src 'self'`
 * with no `unsafe-inline`, so a width written into the markup is blocked and
 * the bar silently renders at its default size — which is exactly what the
 * first draft of these three bars did, and what the browser drive caught.
 * Assigning the property is not inline style for CSP's purposes, which is
 * why the tooltip has positioned itself this way since 0.11.0. */
function sized(property, share, attrs) {
  const node = el("span", attrs || {});
  node.style[property] = `${Math.max(0, Math.min(100, share * 100))}%`;
  return node;
}

function healthPanel(options) {
  // `languages: false` for a page that draws the mix itself, by line rather
  // than by file. Two language cards on one page are two answers to one
  // question, and the reader cannot tell which is which.
  const withLanguages = !options || options.languages !== false;
  // A placeholder rather than an empty element: the read is a scan of every
  // call edge, so on a large store the three cards are a second or two away,
  // and a gap where a card will be reads as a page that has finished.
  const node = el(
    "div",
    { class: "health" },
    // Three cards are coming, so three cards of skeleton: the read is a
    // scan of every call edge and on a large store it is a second or two.
    withLanguages ? el("section", { class: "health-card" }, skeletonRows(5)) : null,
    el("section", { class: "health-card" }, skeletonRows(4)),
    el("section", { class: "health-card" }, skeletonRows(6)),
  );
  let rows = [];

  async function refresh() {
    try {
      const data = await api("/api/stores?coverage=1");
      rows = data.stores || [];
    } catch {
      rows = [];
    }
    paint();
  }

  function bar(counts) {
    const total = counts.reduce((sum, [, n]) => sum + n, 0);
    if (!total) return el("div", { class: "rail-hint", text: "No call edges yet." });
    return el(
      "div",
      { class: "support-bar", role: "img", "aria-label": counts.map(([k, n]) => `${k} ${n}`).join(", ") },
      // Each segment carries its own reading. The bar says the proportion at a
      // glance; the number behind a proportion is the thing a reader asks for
      // next, and reading it off a 6px band is not something anyone can do.
      counts.map(([kind, count]) =>
        count
          ? sized("width", count / total, {
              class: `seg ${kind}`,
              tabindex: "0",
              "data-tip": `${kind} · ${n(count)} edge${count === 1 ? "" : "s"} · ${share(count, total)} of ${n(total)}`,
            })
          : null,
      ),
    );
  }

  function paint() {
    // The four support classes a coverage row carries. `unresolved` is an
    // edge whose target this store holds no definition for — the fourth
    // segment of the bar, and a different thing from an ambiguous one.
    const counts = { extracted: 0, resolved: 0, ambiguous: 0, unresolved: 0 };
    const languages = [];
    let unresolvedNames = 0;
    let several = 0;
    const top = [];
    const months = new Map();
    for (const store of rows) {
      for (const row of store.coverage || []) {
        if (!row.definitions && !row.files) continue;
        counts.extracted += row.extracted || 0;
        counts.resolved += row.resolved || 0;
        counts.ambiguous += row.ambiguous || 0;
        counts.unresolved += row.unresolved || 0;
        languages.push({ store: store.name, ...row });
      }
      const health = store.health;
      if (!health) continue;
      unresolvedNames += health.unresolved_names || 0;
      several += health.several_definitions || 0;
      for (const one of health.unresolved_top || []) top.push(one);
      for (const one of health.months || []) {
        months.set(one.month, (months.get(one.month) || 0) + one.chunks);
      }
    }
    /* Across the corpus, not per store.
     *
     * The coverage rows arrive one per store per language, and drawn straight
     * they made a list that read `rust · rust · markdown · markdown · rust`,
     * with the same name three times and no way to tell the rows apart. The
     * page is about what this machine holds, so the languages are summed the
     * way the design sums them, and each row names the stores behind it. The
     * same applies to the unresolved names: one call target reached from two
     * stores is one name, not two rows. */
    const merge = (into, key, row, fields) => {
      const at = into.get(key) || { ...row, stores: new Set() };
      if (into.has(key)) for (const field of fields) at[field] = (at[field] || 0) + (row[field] || 0);
      at.stores.add(row.store);
      into.set(key, at);
      return at;
    };
    const perLanguage = new Map();
    for (const row of languages) {
      merge(perLanguage, row.language, row, [
        "files",
        "definitions",
        "extracted",
        "resolved",
        "ambiguous",
        "unresolved",
        "unparsed",
      ]);
    }
    const merged = [...perLanguage.values()];
    const perName = new Map();
    for (const one of top) merge(perName, one.name, { ...one, store: one.store || "" }, ["edges"]);
    const topNames = [...perName.values()].sort((a, b) => b.edges - a.edges);

    const carrying = merged.filter((row) => row.extracted || row.resolved || row.ambiguous);
    const byFiles = [...merged].sort((a, b) => b.files - a.files).slice(0, 8);
    const filesTotal = merged.reduce((sum, row) => sum + row.files, 0) || 1;
    const monthRows = [...months.entries()].sort((a, b) => a[0].localeCompare(b[0])).slice(-12);
    const peak = Math.max(1, ...monthRows.map(([, count]) => count));
    const monthTotal = monthRows.reduce((sum, [, count]) => sum + count, 0);

    fill(
      node,
      !withLanguages ? null : el(
        "section",
        { class: "health-card" },
        el("span", { class: "eyebrow", text: "Language mix" }),
        byFiles.length
          ? el(
              "div",
              { class: "mix" },
              byFiles.map((row) =>
                el(
                  "div",
                  {
                    class: "mix-row",
                    tabindex: "0",
                    "data-tip": `${row.language} · ${n(row.files)} file${row.files === 1 ? "" : "s"} · ${share(
                      row.files,
                      filesTotal,
                    )} of ${n(filesTotal)} indexed · ${[...row.stores].sort().join(", ")}`,
                  },
                  el("span", { class: "k", text: row.language }),
                  el("span", { class: "meter" }, sized("width", row.files / filesTotal)),
                  el("span", { class: "v", text: `${n(row.files)} file${row.files === 1 ? "" : "s"}` }),
                ),
              ),
            )
          : el("div", { class: "rail-hint", text: "Nothing indexed yet." }),
      ),
      el(
        "section",
        { class: "health-card" },
        el("span", { class: "eyebrow", text: "Chunks by month indexed" }),
        monthRows.length
          ? el(
              "div",
              { class: "months" },
              monthRows.map(([month, count]) =>
                el(
                  "div",
                  {
                    class: "month",
                    tabindex: "0",
                    // The column's own label is the month abbreviated to fit
                    // under a 44px bar; the tip is where the whole month, the
                    // count and its share of the year go.
                    "data-tip": `${month} · ${n(count)} chunk${count === 1 ? "" : "s"} · ${share(
                      count,
                      monthTotal,
                    )} of the ${n(monthTotal)} in this window · ${share(count, peak)} of the busiest month`,
                  },
                  el("span", { class: "col" }, sized("height", count / peak)),
                  el("span", { class: "m", text: month.slice(2) }),
                  el("span", { class: "c", text: n(count) }),
                ),
              ),
            )
          : el("div", { class: "rail-hint", text: "No files indexed yet." }),
        // Said rather than implied: the store records when it read a file,
        // not when anybody wrote it.
        el("p", { class: "note", text: "When semlith read the file, not when it was written." }),
      ),
      el(
        "section",
        { class: "health-card" },
        el("span", { class: "eyebrow", text: "Graph health" }),
        bar([
          ["extracted", counts.extracted],
          ["resolved", counts.resolved],
          ["ambiguous", counts.ambiguous],
          ["unresolved", counts.unresolved],
        ]),
        el(
          "div",
          { class: "health-facts" },
          el("div", { class: "fact" }, el("span", { class: "v", text: n(unresolvedNames) }), el("span", { class: "k", text: "call targets with no definition here" })),
          el("div", { class: "fact" }, el("span", { class: "v", text: n(several) }), el("span", { class: "k", text: "names with several definitions" })),
          el("div", { class: "fact" }, el("span", { class: "v", text: `${carrying.length} of ${languages.length}` }), el("span", { class: "k", text: "languages carrying edges" })),
        ),
        topNames.length
          ? el(
              "div",
              { class: "mix" },
              topNames.slice(0, 5).map((one) =>
                el(
                  "div",
                  {
                    class: "mix-row",
                    tabindex: "0",
                    "data-tip": `${one.name} · ${n(one.edges)} call${
                      one.edges === 1 ? "" : "s"
                    } reach a name no store here holds a definition for · ${[...one.stores]
                      .filter(Boolean)
                      .sort()
                      .join(", ") || "this store"}`,
                  },
                  el("span", { class: "k", text: one.name }),
                  el("span", { class: "v", text: `${n(one.edges)} edge${one.edges === 1 ? "" : "s"}` }),
                ),
              ),
            )
          : null,
      ),
    );
  }

  refresh();
  return { node, paint: refresh };
}

function coveragePanel() {
  const node = el("div", { class: "rows" });
  // Its own fetch, and only this page makes it: the figures are a scan of
  // every call edge, so the shared poll must not carry them to every page
  // that happens to want a store count.
  let rows = [];
  async function refresh() {
    try {
      const data = await api("/api/stores?coverage=1");
      rows = data.stores || [];
    } catch {
      rows = [];
    }
    paint();
  }
  function paint() {
    const coverage = [];
    for (const store of rows) {
      for (const row of store.coverage || []) {
        if (!row.definitions && !row.files) continue;
        coverage.push({ store: store.name, ...row });
      }
    }
    fill(
      node,
      coverage.length
        ? dataTable({
            className: "w-coverage",
            caption:
              "What the graph covers, per language: files indexed, files the parser gave up on, definitions, and call edges by how firmly each one landed.",
            sort: "files",
            dir: "desc",
            rows: coverage,
            columns: [
              { key: "store", label: "Store", className: "meta narrow-drop", value: (r) => r.store, render: (r) => r.store },
              { key: "language", label: "Language", value: (r) => r.language, render: (r) => r.language },
              { key: "files", label: "Files", className: "num", value: (r) => r.files, render: (r) => String(r.files) },
              {
                key: "parser_failed",
                label: "Unparsed",
                className: "num",
                value: (r) => r.parser_failed,
                render: (r) =>
                  el("span", {
                    class: r.parser_failed ? "bad" : "meta",
                    title: "Files whose parse expired, so the store holds their text and none of their structure.",
                    text: String(r.parser_failed),
                  }),
              },
              { key: "definitions", label: "Definitions", className: "num", value: (r) => r.definitions, render: (r) => String(r.definitions) },
              { key: "extracted", label: "Extracted", className: "num", value: (r) => r.extracted, render: (r) => String(r.extracted) },
              { key: "resolved", label: "Resolved", className: "num", value: (r) => r.resolved, render: (r) => String(r.resolved) },
              { key: "ambiguous", label: "Ambiguous", className: "num", value: (r) => r.ambiguous, render: (r) => String(r.ambiguous) },
              {
                key: "unresolved",
                label: "Unresolved",
                className: "num",
                value: (r) => r.unresolved,
                title: "Call targets no definition in this store satisfies.",
                render: (r) => String(r.unresolved),
              },
              {
                key: "settled",
                label: "Settled",
                className: "num",
                value: (r) => r.settled,
                render: (r) => `${r.settled} %`,
              },
            ],
          }).node
        : empty("No language holds a definition yet. Index a folder of code and the graph fills in."),
    );
  }
  paint();
  refresh();
  return { node, paint: refresh };
}

/* The Index page.
 *
 * Until 0.20.0 this page *was* the run: it held the streaming response open,
 * and the run existed only as the events travelling down it. Leaving the page
 * threw that away — the work carried on, because the work is the store's, but
 * nothing on screen could ever find it again.
 *
 * The run lives in the daemon now, so this page is a reader. On mount it asks
 * `/api/index/runs` for every store's run and draws one card each; the shared
 * live poll tells it when to ask again; each card pulls its own log lines
 * after its own cursor. Nothing here outlives a poll, so navigating away,
 * refreshing, or closing the tab for the length of a run changes nothing the
 * page shows when it comes back.
 */

/** How long something took: `0.4s`, `42s`, `5m 18s`, `1h 02m`. */
function spellTook(ms) {
  if (ms < 100) return "under 0.1s";
  if (ms < 1000) return `${(ms / 1000).toFixed(1)}s`;
  const all = Math.round(ms / 1000);
  if (all < 60) return `${all}s`;
  if (all < 3600) return `${Math.floor(all / 60)}m ${String(all % 60).padStart(2, "0")}s`;
  return `${Math.floor(all / 3600)}h ${String(Math.floor(all / 60) % 60).padStart(2, "0")}m`;
}

/** What is left of an estimate, in the rounded words a guess deserves:
 * `about 3 min left`, `about 40 s left`, `almost done`. */
function spellLeft(ms) {
  const seconds = Math.round(ms / 1000);
  if (seconds < 10) return "almost done";
  if (seconds < 60) return `about ${Math.round(seconds / 5) * 5} s left`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 90) return `about ${minutes} min left`;
  return `about ${Math.floor(minutes / 60)} h ${String(minutes % 60).padStart(2, "0")} min left`;
}

/** How long a run has taken, as `12:34` or `1:02:03`. */
/** Chunks a second, to one decimal under ten so a slow lane reads as "2.4"
 * rather than rounding to nothing. */
function perSecond(value) {
  return value >= 10 ? n(Math.round(value)) : Number(value).toFixed(1);
}

function spell(ms) {
  const all = Math.max(0, Math.round(ms / 1000));
  const seconds = String(all % 60).padStart(2, "0");
  const hours = Math.floor(all / 3600);
  const minutes = String(Math.floor(all / 60) % 60).padStart(2, "0");
  return hours ? `${hours}:${minutes}:${seconds}` : `${minutes}:${seconds}`;
}

const RUN_TONE = {
  queued: "warn",
  review: "warn",
  running: "good",
  pausing: "warn",
  paused: "warn",
  held: "warn",
  stopping: "warn",
  done: "good",
  stopped: "bad",
  failed: "bad",
};

const RUN_WORD = {
  queued: "queued",
  // Scanned, and waiting for a person before anything is embedded (2.7).
  review: "waiting for review",
  running: "indexing",
  // Said the moment Pause is pressed: the engine stops at its next batch, and
  // a button that answers nothing until then reads as a button that failed.
  pausing: "pausing",
  paused: "paused",
  // Admitted once and held again because `runs at once` was lowered. It is
  // not paused and it has not lost anything, and the pill says both.
  held: "held — waiting for a slot, keeps its progress",
  stopping: "stopping",
  done: "done",
  stopped: "stopped",
  failed: "failed",
};

/** One store's run: its bar, its counts, its log, and its own two controls. */
function runCard(run, controls) {
  const bar = el("span", {});
  const track = el("div", { class: "bar", tabindex: "0" }, bar);
  const pct = el("span", { class: "pct" });
  /* The run's reading, one line of text with its parts as their own nodes, so
   * a poll moves the words that moved and nothing else — and a field the
   * daemon adds is one more node here and one `setText` in `absorb`. */
  const counts = document.createTextNode("");
  // Each starts holding an empty text node, so its first words are an edit
  // of that node rather than a child added under a card being watched.
  const rate = el("span", {}, "");
  const lanes = el("span", {}, "");
  const threads = el("span", {}, "");
  const status = el("span", { class: "meta" }, counts, rate, lanes, threads);
  const elapsed = el("span", { class: "meta" });
  const log = el("div", { class: "log", "aria-live": "polite" });
  // What a refused control said, on the card it was pressed on.
  const problem = el("div", { class: "note bad" }, "");
  // What the run's stop did to its store, when it was asked to delete it.
  const outcome = el("div", { class: "note" }, "");
  /* The scan phase's plan (2.7), on one line. What a person decides about it
   * is the page's scan panel above the cards, not this card's.
   *
   * Its node is made once and only its words and `hidden` change after, so
   * no poll rebuilds the line (drive finding 8.3). */
  const planLine = el("div", { class: "meta run-plan", hidden: "" }, "");
  function paintPlan(next) {
    const plan = next.plan;
    planLine.hidden = !plan;
    if (!plan) return;
    const not = Object.values(plan.not_indexed || {}).reduce((a, b) => a + b, 0);
    const review = (plan.review || []).length;
    setText(
      planLine,
      next.status === "review"
        ? `scanned: ${n(plan.embed)} to embed (${bytes(plan.embed_bytes)}) · ${n(plan.unchanged)} unchanged · waiting for Start indexing`
        : `plan: ${n(plan.embed)} to embed (${bytes(plan.embed_bytes)}) · ${n(plan.unchanged)} unchanged · ${n(not)} not indexed · ${n(review)} needed review`,
    );
  }

  /** The last reading, so a control's answer can move the card before the
   * next poll does. */
  let last = run;
  const badge = pill("", "good");
  // `pill` writes its text as the last child, after the dot.
  const badgeWord = badge.lastChild;
  const where = el("span", { class: "meta" });

  const pause = el("button", { class: "button secondary small", type: "button" });
  const stop = el("button", { class: "button secondary small", type: "button" });
  const remove = el("button", {
    class: "button secondary small",
    type: "button",
    text: "Remove",
    title: "Dismiss this card. The store is not touched.",
  });
  /* A finished card folds to its one line until someone opens it. Four
   * finished runs used to be four full cards with their whole logs, and the
   * live one was appended below them, off the bottom of the screen. */
  const expand = el("button", {
    class: "button secondary small",
    type: "button",
    "aria-expanded": "false",
  });
  let open = true;

  /* What the clock says depends on where the run is. Running: the time
   * left, which the daemon estimates from the bytes still to go and counts
   * down here between polls; `estimating…` until its rate settles. Queued:
   * how long it has waited. Finished: how long the work took, from its start
   * rather than its submission, and when it ended, with any wait named apart.
   * The person watching a run wants to know when it will be done, and a clock
   * counting up from the moment they pressed the button answered a different
   * question. */
  let shown = 0;
  let readAt = 0;
  let ticking = false;

  function paintClock() {
    const since = Date.now() - readAt;
    const next = last;
    let text = "";
    if (next.status === "queued") {
      text = `waiting ${spellTook(ticking ? shown + since : shown)}`;
    } else if (next.status === "running") {
      text = next.eta_ms === null || next.eta_ms === undefined ? "estimating…" : spellLeft(next.eta_ms - since);
    } else if (next.finished_at && next.started_at) {
      // The run's own clock, which excludes time held, minus its wait in the
      // queue: the work, to the millisecond.
      const queued = next.queued_ms || 0;
      text = `took ${spellTook(Math.max(0, shown - queued))} · finished ${new Date(next.finished_at * 1000)
        .toTimeString()
        .slice(0, 5)}${queued >= 1000 ? ` · queued ${spellTook(queued)}` : ""}`;
    }
    setText(elapsed, text);
    elapsed.hidden = !text;
  }

  function absorb(next) {
    last = next;
    paintPlan(next);
    shown = next.elapsed_ms || 0;
    readAt = Date.now();
    ticking = !!next.ticking;
    paintClock();

    // Not `share`: that is the helper every bar's tooltip uses to say what
    // fraction of a whole it is, and a local of the same name would shadow it
    // inside this function only, which is the kind of bug that survives review.
    const scanned = next.total ? Math.min(100, (next.scanned / next.total) * 100) : 0;
    const finished = next.status === "done";
    // A stopped run undid everything it embedded, so a full bar would say the
    // opposite of what happened.
    const width = finished ? 100 : next.status === "stopped" ? 0 : scanned;
    bar.style.width = `${width.toFixed(1)}%`;
    setText(pct, `${Math.round(width)}%`);
    // What the bar is a bar of. The card states files and chunks elsewhere;
    // the bar itself said only a percentage of something unnamed.
    track.setAttribute(
      "data-tip",
      next.status === "stopped"
        ? "stopped · everything this run embedded was undone"
        : `${n(next.scanned || 0)} of ${n(next.total || 0)} file${next.total === 1 ? "" : "s"} scanned · ${n(
            next.chunks || 0,
          )} chunk${next.chunks === 1 ? "" : "s"} written · ${Math.round(width)}%`,
    );

    // The pill's own text node, beside its dot. It used to be rebuilt dot and
    // all on every poll, which replaced two children a second on every card.
    // A catch-up is the watcher's run rather than somebody's, and says so.
    setText(
      badgeWord,
      next.status === "queued" && next.position
        ? `queued · ${next.position} in line`
        : next.kind === "catch-up" && next.status === "running"
          ? "catching up"
          : RUN_WORD[next.status] || next.status,
    );
    const tone = `pill ${RUN_TONE[next.status] || "warn"}`;
    if (badge.className !== tone) badge.className = tone;

    const live = TICKING.has(next.status);
    // The phase, when the run is doing something other than reading files.
    // Twenty seconds of a bar not moving is a hang unless the card says what
    // it is: every two hundred files the run rewrites its shards.
    setText(
      counts,
      next.phase
        ? `${n(next.scanned)}/${n(next.total)} files · ${next.phase} · `
        : next.total
          ? `${n(next.scanned)}/${n(next.total)} files · ${n(next.chunks)} chunks · `
          : `${RUN_WORD[next.status] || next.status} · `,
    );
    /* The daemon's rolling rate, over the last ten seconds of work, on every
     * poll while the run is live — "—" until the first batch has given it one.
     * It used to be chunks over the whole elapsed clock, which counted the
     * time a run sat queued or paused against it. A finished run keeps its
     * average, which is the one reading that is still true of it. */
    const reading = live ? next.rate : next.rate_average;
    // A run still in the line has read nothing, so it has no rate to show,
    // and a finished run that never embedded a batch has no average either.
    rate.hidden = next.status === "queued" || (!live && (reading === null || reading === undefined));
    setText(rate, `${reading === null || reading === undefined ? "—" : perSecond(reading)} chunks/s · `);
    const average = next.rate_average;
    const tip = average === null || average === undefined ? "" : `${perSecond(average)} chunks/s on average since the first batch`;
    if (rate.title !== tip) rate.title = tip;
    // Which device is doing what, once there is more than one doing it.
    const split = Object.entries(next.lane_rates || {}).sort((a, b) => b[1] - a[1]);
    lanes.hidden = split.length < 2;
    setText(lanes, split.length < 2 ? "" : `${split.map(([lane, r]) => `${lane.toUpperCase()} ${n(Math.round(r))}/s`).join(" · ")} · `);
    threads.hidden = !next.threads;
    setText(threads, next.threads ? `${n(next.threads)} thread${next.threads === 1 ? "" : "s"} · ` : "");
    setText(where, (next.paths || []).join(", "));
    const deleted = next.delete || "";
    setText(outcome, deleted);
    const outcomeClass = deleted.startsWith("the store was not deleted") ? "note bad" : "note";
    if (outcome.className !== outcomeClass) outcome.className = outcomeClass;

    // Pausing reads as held already: the button offers the way back.
    const held = next.status === "paused" || next.status === "pausing";
    // In place, so the button somebody just pressed is still the focused one
    // when it turns from Pause to Resume.
    setText(pause, held ? "Resume" : "Pause");
    // Nothing to pause in a run that has not started or is held for a slot.
    pause.hidden = !live || next.status === "queued" || next.status === "held" || next.status === "review";
    // A queued run has embedded nothing, so taking it out of the line costs
    // nothing and is not the same act as stopping one that is going.
    // "Take out of the queue" rather than "Remove": a finished card's Remove
    // dismisses the card and touches nothing, and one word for both put two
    // different acts on one page under one label.
    setText(stop, next.status === "queued" ? "Take out of the queue" : "Stop");
    stop.hidden = !live;
    stop.disabled = next.status === "stopping";
    // A run that has finished has nothing to pause and nothing to stop. It
    // used to offer Stop, whose confirm promised to undo everything the run
    // had embedded; pressing it did nothing visible and left the store
    // carrying a cancellation that killed the next run against it at 0%.
    remove.hidden = live;

    // Finished cards start folded, live ones start open, and whatever the
    // reader has chosen since is kept.
    if (touched === false) {
      open = live;
      touched = null;
    }
    paintFold();
  }

  function paintFold() {
    log.hidden = !open;
    where.hidden = !open;
    setText(expand, open ? "Hide the log" : "Show the log");
    expand.setAttribute("aria-expanded", String(open));
  }

  /* `false` until the first reading has decided the default; `null` once the
   * reader has taken it over. */
  let touched = false;

  pause.addEventListener("click", () => controls.hold(pause.textContent === "Pause"));
  stop.addEventListener("click", () => controls.stop(stop.textContent !== "Stop"));
  remove.addEventListener("click", () => controls.remove());
  expand.addEventListener("click", () => {
    open = !open;
    touched = null;
    paintFold();
    // Its lines are wanted now. A folded card's log is not on screen, so it is
    // not fetched until it is — which is what keeps a page of finished cards
    // from costing one request each on the first paint.
    if (open && controls.reveal) controls.reveal();
  });

  const node = el(
    "div",
    { class: "card pad run-card" },
    el(
      "div",
      { class: "head" },
      el("span", { class: "card-title", text: run.store }),
      badge,
      el("span", { class: "spacer" }),
      pct,
    ),
    planLine,
    track,
    el(
      "div",
      { class: "filters" },
      // The clock sits with the counts it belongs to. Files, chunks, rate and
      // elapsed are one reading of one run; across the card from them the
      // clock read as a property of the page rather than of the work.
      status,
      elapsed,
      el("span", { class: "spacer" }),
      expand,
      pause,
      stop,
      remove,
    ),
    problem,
    outcome,
    where,
    log,
  );

  absorb(run);
  const clock = setInterval(() => {
    if (!node.isConnected) return clearInterval(clock);
    if (ticking) paintClock();
  }, 1000);

  return { node, absorb, log, problem, last: () => last, isOpen: () => open };
}

/** Append one log line, keeping the reader's place if they have scrolled up. */
function logLine(log, event) {
  const outcome = event.outcome || null;
  const key =
    event.event === "file" ? `${event.scanned}/${event.total}` : event.event || "";
  const text =
    event.event === "file"
      ? event.why
        ? `${event.path} — ${event.why}`
        : event.path
      : logText(event);
  log.append(
    el(
      "div",
      { class: outcome ? `line ${outcome}` : "line" },
      key ? el("span", { class: "key", text: key }) : null,
      outcome ? el("span", { class: "outcome", text: outcome }) : null,
      el("span", { class: "what", text }),
    ),
  );
  // Only while the reader is already at the end: scrolling back through a long
  // run should not be yanked away by the next line.
  const atEnd = log.scrollHeight - log.scrollTop - log.clientHeight < 40;
  if (atEnd) log.scrollTop = log.scrollHeight;
  while (log.childElementCount > 500) log.firstElementChild.remove();
}

/** What a non-file event says on the log. */
function logText(event) {
  switch (event.event) {
    case "submitted":
      return event.ahead
        ? `waiting — ${n(event.ahead)} run${event.ahead === 1 ? "" : "s"} ahead of this one`
        : "submitted";
    case "queued":
      return event.ahead
        ? `waiting for ${event.store}'s writer — ${n(event.ahead)} job${
            event.ahead === 1 ? "" : "s"
          } ahead of this one`
        : `waiting for ${event.store}'s writer; it finishes what the watcher is doing first`;
    case "started":
      return "walking the tree and hashing what it finds";
    case "slice":
      return `${n(event.remaining)} paths left; the writer is giving the watcher a turn and will carry on`;
    case "paused":
      return "held between files — the writer is still this run's";
    case "resumed":
      return "carrying on";
    case "error":
      return event.error || "failed";
    case "done":
      if (event.dequeued) return "removed from the queue before it started; nothing was indexed";
      if (event.stopped) {
        return "stopped — everything this run embedded was undone, so the store is as it was before it started";
      }
      return `${n(event.indexed)} indexed, ${n(event.unchanged)} unchanged, ${n(
        event.skipped,
      )} skipped, ${n(event.removed)} removed, ${n(event.chunks)} chunks · ${spell(
        event.elapsed_ms || 0,
      )}`;
    default:
      return event.event || "";
  }
}

/** One of the three settings, with what the machine derived and why. */
/** Why a given setting stops where it does, in the machine's own terms. */
function capped(limit) {
  return limit.ceiling === limit.derived
    ? "It is already what this machine derives."
    : "Past it the machine would be promising work it cannot carry.";
}

/* The moving free-memory figure, taken out of a limit's reason.
 *
 * The daemon writes how much memory is free now into two of the three
 * reasons, and that figure moves every second. A sentence that changes every
 * second under a field is a sentence nobody can read, and it re-wrapped the
 * card while it was being typed into. The figure is on the card once, in the
 * subtitle, where it updates in place; the reasons keep the rule and lose the
 * reading.
 *
 * ponytail: rewrites the daemon's prose (`derive` in src/system.rs), so a
 * rewording there makes these two patterns miss and the figure comes back in
 * the reason — visibly, and harmlessly. The durable fix is the daemon not
 * writing the figure into the reason, and this goes when it does. */
function steady(reason) {
  return String(reason || "")
    .replace(/^[\d.]+ [GM]iB free (?=minus|is under)/, "free memory ")
    .replace(/, and (?:none|[\d.]+ [GM]iB) is free beyond the reserve out of the [\d.]+ [GM]iB free now$/, "");
}

function settingField(key, label, limit, onSave) {
  /* The limit in force, replaced by `update` on every poll. The field is
   * built once and patched, never rebuilt: a field rebuilt under the cursor
   * cannot be typed into. */
  let current = limit;
  /* Typed into and not yet saved. A dirty field is the reader's, and the poll
   * leaves its value alone until `change` hands it to the daemon. */
  let dirty = false;
  const input = el("input", {
    type: "number",
    min: "1",
    "aria-label": label,
  });
  const why = el("div", { class: "note" });

  /* What the field says under itself.
   *
   * None of this is a warning any more. Changing these is the ordinary thing
   * to do with them — a laptop doing nothing else can index harder than the
   * default — and the panel used to answer every such change in red, which
   * reads as "you have broken something" rather than "here is what that
   * means". The only genuinely constrained case is the ceiling, and the field
   * will not go past it, so there is nothing left to warn about.
   */
  function explain() {
    const limit = current;
    const asked = Number(input.value);
    // Where the value in force came from, on every branch. A saved value above
    // the derived one takes the second branch every time, so a branch that did
    // not say "saved" said nothing about it in exactly the case where a user is
    // trying to work out whether their setting took effect — while the daemon's
    // own line calls the same value saved.
    const held = limit.source === "saved" && asked === limit.value ? `${limit.value} saved. ` : "";
    if (limit.source === "environment") {
      setText(why, `Set in this daemon's environment, so the page leaves it alone. This machine would derive ${limit.derived} — ${limit.reason}`);
      return;
    }
    if (asked >= limit.ceiling) {
      // Not red either. It is the top of the range, which is a fact about the
      // machine rather than a mistake by the person.
      setText(why, `${held}${limit.ceiling} is as high as this machine goes. ${capped(limit)}`);
      return;
    }
    // The memory ceiling is what is free now less the reserve, so as a number
    // it moves every second like the free figure it comes from. It is said as
    // the rule here; the input's `max` still carries the number.
    const room = key === "index_memory_mb" ? "what is free now, less the reserve," : `the ${limit.ceiling}`;
    if (asked > limit.derived) {
      setText(why, `${held}Above the ${limit.derived} this machine would pick on its own, and under ${room} it will allow — ${limit.reason} Yours to set.`);
      return;
    }
    const from =
      limit.source === "saved"
        ? `${limit.value} saved, and this machine derives ${limit.derived}`
        : `${limit.derived}, derived`;
    setText(
      why,
      `${from} — ${limit.reason} Room up to ${key === "index_memory_mb" ? "what is free now, less the reserve" : limit.ceiling}.`,
    );
  }

  /** Take the daemon's latest reading, touching only what it moved. */
  function update(next) {
    current = { ...next, reason: steady(next.reason) };
    // The field will not go above what the machine will accept, and the route
    // clamps it as well — a `max` on an input is a courtesy, not a control.
    const max = String(current.ceiling);
    if (input.max !== max) input.max = max;
    const fixed = current.source === "environment";
    if (input.disabled !== fixed) input.disabled = fixed;
    const value = String(current.value);
    if (!dirty && document.activeElement !== input && input.value !== value) input.value = value;
    explain();
  }

  input.addEventListener("input", () => {
    dirty = true;
    explain();
  });
  input.addEventListener("change", () => {
    const asked = Math.min(current.ceiling, Math.max(1, Number(input.value) || 1));
    input.value = String(asked);
    dirty = false;
    explain();
    onSave(key, asked);
  });
  update(limit);

  return {
    update,
    node: el(
      "div",
      { class: "setting" },
      el("div", { class: "field" }, el("span", { class: "prefix", text: label }), input),
      why,
    ),
  };
}

/* The accelerator lanes, on the Machine limits card.
 *
 * One row per lane — the CPU, WebGPU, CUDA, and the worker when it is on —
 * each the row-wide switch the Privacy page's replay control already is, with
 * the lane's device, where it stands and its share of the rate. Built once and
 * patched from `/api/accel`, which the Index page reads with its runs.
 *
 * The daemon refuses what it will not do — the CPU off with no GPU lane able
 * to carry the work, CUDA anywhere but Linux — with a 409 that says why, and
 * that sentence is shown as it came, on this card. */
const LANE_NAMES = { cpu: "CPU", gpu: "GPU", cuda: "CUDA", worker: "Worker" };

/** A size in the binary units the Machine limits card already counts in: its
 * memory field is "MiB per store", and one card with two units for sizes is
 * what finding 3.2 was about. */
function binarySize(value) {
  const mib = (Number(value) || 0) / 1048576;
  return mib >= 1024 ? `${(mib / 1024).toFixed(1)} GiB` : `${mib.toFixed(1)} MiB`;
}

function laneState(status) {
  const state = (status && status.state) || "idle";
  if (state === "downloading") return `downloading ${status.percent ?? 0}%`;
  if (status && status.reason) return `${state} — ${status.reason}`;
  return state;
}

function accelSection() {
  const rows = el("div", { class: "rows accel-lanes" });
  const fallback = el("p", { class: "note" });
  const problem = el("div", { class: "note" });
  const drawn = new Map();
  let data = null;
  let asked = 0;
  let painted = 0;

  function say(text, bad) {
    problem.className = bad ? "note bad" : "note";
    setText(problem, text);
  }

  async function change(lane, action) {
    try {
      const answer = await post("/api/accel", { lane, action });
      say(answer.said || "");
    } catch (e) {
      say(e.message, true);
    }
    await refresh();
  }

  function row(lane) {
    const knob = el("span", { class: "knob", "aria-hidden": "true" });
    const title = el("span", { class: "replay-state" });
    const where = el("span", { class: "replay-switch-note" });
    const share = el("span", { class: "meta" });
    const toggle = el(
      "button",
      { class: "replay-switch", type: "button", role: "switch", "aria-checked": "false" },
      knob,
      el("span", { class: "replay-switch-text" }, title, where),
      el("span", { class: "spacer" }),
      share,
    );
    const remove = el("button", { class: "button secondary small", type: "button" });
    const removeRow = el("div", { class: "filters accel-remove" }, remove);
    const drawnRow = { title, where, share, toggle, remove, removeRow, enabled: false, node: null };
    toggle.addEventListener("click", () => {
      const on = !drawnRow.enabled;
      // A download of that size is somebody's decision, made knowing it.
      if (on && lane === "cuda") {
        ask({
          title: "Turn CUDA on?",
          body: `It downloads the CUDA pack first, ${binarySize(data?.bytes?.cuda_download || 0)}, once, into this machine's model cache. Runs use it from their next batch.`,
          confirm: "Download and turn on",
          run: () => change(lane, "on"),
        });
        return;
      }
      change(lane, on ? "on" : "off");
    });
    remove.addEventListener("click", () => change(lane, "remove"));
    drawnRow.node = el("div", { class: "accel-lane" }, toggle, removeRow);
    return drawnRow;
  }

  function paint(next) {
    data = next;
    const seen = new Set();
    for (const lane of next.lanes || []) {
      seen.add(lane.lane);
      let r = drawn.get(lane.lane);
      if (!r) {
        r = row(lane.lane);
        drawn.set(lane.lane, r);
      }
      r.enabled = !!lane.enabled;
      r.toggle.classList.toggle("on", r.enabled);
      const checked = String(r.enabled);
      if (r.toggle.getAttribute("aria-checked") !== checked) r.toggle.setAttribute("aria-checked", checked);
      const name = LANE_NAMES[lane.lane] || lane.lane;
      setText(r.title, `${name} · ${lane.device || "no device found"}${lane.variant ? ` · ${lane.variant}` : ""}`);
      setText(r.where, `${r.enabled ? "" : "off · "}${laneState(lane.status)}`);
      setText(r.share, `${Math.round(lane.share || 0)} %`);
      const held = (next.bytes || {})[lane.lane] || 0;
      r.removeRow.hidden = !(held > 0 && (lane.lane === "gpu" || lane.lane === "cuda"));
      setText(r.remove, `Remove downloaded files (${binarySize(held)})`);
    }
    for (const [lane, r] of drawn) {
      if (seen.has(lane)) continue;
      r.node.remove();
      drawn.delete(lane);
    }
    arrange(rows, (next.lanes || []).map((lane) => drawn.get(lane.lane).node));
    setText(fallback, next.cpu_fallback ? "The CPU is carrying the work: no GPU lane can." : "");
  }

  /** Read the lanes again. Numbered like the runs, so a late answer to an
   * older question is not painted over a newer one. */
  async function refresh() {
    const mine = ++asked;
    let next;
    try {
      next = await api("/api/accel");
    } catch (_) {
      return;
    }
    if (mine < painted) return;
    painted = mine;
    paint(next);
  }

  return {
    refresh,
    node: el(
      "div",
      { class: "rows" },
      el("span", { class: "eyebrow", text: "Accelerators" }),
      fallback,
      rows,
      problem,
    ),
  };
}

/** A path under its store's root, the way the store names it: the
 * machine's absolute path is on the tooltip, not in the column. */
function rootRel(path, storeName) {
  // One spelling for both sides: Windows paths arrive with backslashes, a
  // `\\?\` verbatim prefix or a drive letter in either case, and a root and
  // a file under it can differ in all three.
  const plainPath = (p) => String(p).replace(/\\/g, "/").replace(/^\/\/\?\//, "");
  const key = (p) => (/^[A-Za-z]:\//.test(p) ? p.toLowerCase() : p);
  const whole = plainPath(path);
  const store = (state.stores || []).find((s) => s.name === storeName);
  const roots = (store?.roots || [])
    .map((root) => root.path && plainPath(root.path))
    .filter(Boolean)
    .sort((a, b) => b.length - a.length);
  for (const root of roots) {
    const prefix = root.endsWith("/") ? root : `${root}/`;
    if (key(whole).startsWith(key(prefix))) return whole.slice(prefix.length);
  }
  return whole;
}

/* What a scan found, before anything is embedded (2.7).
 *
 * The runs the Scan button started hold after their scan; this draws every
 * held run as one answer: a summary of what indexing will do, then the files
 * that are a person's to decide, one row each with its own buttons, then one
 * line for what is not indexed and needs nothing. Drawn from the runs the
 * poll already reads, so a reload, a second tab or a scan started elsewhere
 * shows the same panel. Rebuilt only when the set of held runs changes: a
 * poll a second must not redraw buttons under the pointer. */
const NO_ACTION = new Set(["unindexable", "excluded", "credential"]);

/** A scan's own duration: milliseconds under a second, where most scans
 * land, and seconds to one decimal above it. */
function spellScan(seconds) {
  return seconds < 1 ? `${Math.max(1, Math.round(seconds * 1000))} ms` : `${seconds.toFixed(1)} s`;
}

function scanPanel() {
  const node = el("div", { class: "card pad scan-panel", hidden: "" });
  let drawnFor = "";
  /** Decisions made on this page before the store's list says so. */
  const decided = new Map();

  async function paint(runs) {
    const held = (runs || []).filter((run) => run.status === "review" && run.plan);
    const key = held.map((run) => run.id).join(",");
    node.hidden = !held.length;
    if (!held.length) {
      drawnFor = "";
      return;
    }
    if (key === drawnFor) return;
    drawnFor = key;
    // What each store already remembers deciding, and the files with no
    // action, both from the one list the store keeps.
    let refused = { stores: [] };
    try {
      refused = await api("/api/refused");
    } catch {
      // The panel still works from the plans; only the no-action list and a
      // decision made in another tab are missing.
    }
    if (key !== drawnFor) return;
    const rowsOf = (store) => (refused.stores || []).find((s) => s.store === store)?.rows || [];
    draw(held, rowsOf);
  }

  function draw(held, rowsOf) {
    const total = (key) => held.reduce((sum, run) => sum + (run.plan[key] || 0), 0);
    const items = held.flatMap((run) =>
      (run.plan.review || []).map((item) => ({ ...item, store: run.store })),
    );
    const noAction = {};
    for (const run of held) {
      for (const [cls, count] of Object.entries(run.plan.not_indexed || {})) {
        if (NO_ACTION.has(cls)) noAction[cls] = (noAction[cls] || 0) + count;
      }
    }
    const eta = held.every((run) => run.plan.eta_ms != null)
      ? held.reduce((sum, run) => sum + run.plan.eta_ms, 0)
      : null;

    const stat = (label, value, hint) =>
      el(
        "div",
        { class: "scan-stat" },
        el("span", { class: "eyebrow", text: label }),
        el("strong", { text: value }),
        hint ? el("span", { class: "meta", text: hint }) : null,
      );
    const summary = el(
      "div",
      { class: "scan-stats" },
      stat("To embed", n(total("embed")), bytes(held.reduce((sum, run) => sum + (run.plan.embed_bytes || 0), 0))),
      stat("Unchanged", n(total("unchanged")), "already indexed"),
      stat("Need a decision", n(items.length), items.length ? "below" : "nothing to decide"),
      stat("Not indexed", n(Object.values(noAction).reduce((a, b) => a + b, 0)), "no action needed"),
      stat("Estimate", eta == null ? "—" : spellTook(eta), eta == null ? "measured once it starts" : "to embed"),
    );
    const perStore = held.length > 1
      ? dataTable({
          className: "w-scan-stores",
          caption: "What the scan found in each folder",
          rows: held,
          columns: [
            { key: "store", label: "Store", value: (run) => run.store, render: (run) => run.store },
            { key: "path", label: "Folder", sortable: false, render: (run) => pathCell((run.paths || [])[0] || "") },
            { key: "embed", label: "To embed", className: "num", value: (run) => run.plan.embed, render: (run) => n(run.plan.embed) },
            { key: "unchanged", label: "Unchanged", className: "num", value: (run) => run.plan.unchanged, render: (run) => n(run.plan.unchanged) },
            { key: "review", label: "Decide", className: "num", value: (run) => (run.plan.review || []).length, render: (run) => n((run.plan.review || []).length) },
          ],
        }).node
      : null;

    const decisionOf = (item) => {
      if (decided.has(`${item.store}\n${item.path}`)) return decided.get(`${item.store}\n${item.path}`);
      const row = rowsOf(item.store).find((r) => r.path === item.path);
      return row && row.accepted ? row.accepted : null;
    };
    const decisionCell = (item) => {
      const cell = el("div", { class: "decide" });
      const paintCell = () => {
        const now = decisionOf(item);
        if (now) {
          fill(
            cell,
            el("span", {
              class: now === "kept" ? "pill" : "pill good",
              text: now === "kept" ? "stays refused" : now === "redacted" ? "accepted, redacted" : "accepted as-is",
            }),
            now === "kept"
              ? el("button", {
                  class: "button secondary small",
                  type: "button",
                  text: "Change",
                  onclick: () => {
                    decided.delete(`${item.store}\n${item.path}`);
                    paintCell();
                  },
                })
              : null,
          );
          return;
        }
        const accept = (mode) => () =>
          reviewOne(item.store, item, mode, () => {
            decided.set(`${item.store}\n${item.path}`, mode);
            paintCell();
          });
        fill(
          cell,
          item.class === "content"
            ? el("button", { class: "button small", type: "button", text: "Accept redacted", onclick: accept("redacted") })
            : null,
          el("button", {
            class: item.class === "content" ? "button secondary small" : "button small",
            type: "button",
            text: item.class === "content" ? "Accept as-is" : "Accept",
            onclick: accept("as-is"),
          }),
          el("button", {
            class: "button secondary small",
            type: "button",
            text: "Keep refused",
            onclick: () => {
              decided.set(`${item.store}\n${item.path}`, "kept");
              paintCell();
            },
          }),
        );
      };
      paintCell();
      return cell;
    };
    const decisions = items.length
      ? dataTable({
          className: "w-scan-review",
          caption: "Files the scan held back, each waiting for a decision",
          rows: items,
          columns: [
            {
              key: "path",
              label: "File",
              value: (item) => rootRel(item.path, item.store),
              render: (item) => pathCell(rootRel(item.path, item.store), "path"),
            },
            held.length > 1 ? { key: "store", label: "Store", className: "meta", value: (item) => item.store, render: (item) => item.store } : null,
            { key: "rule", label: "Why", sortable: false, className: "narrow-drop", render: (item) => lineCell(item.rule) },
            {
              key: "confidence",
              label: "Likely real",
              className: "num",
              value: (item) => (item.confidence == null ? -1 : item.confidence),
              render: (item) => (item.confidence == null ? "—" : `${item.confidence} %`),
            },
            { key: "decide", label: "Decision", sortable: false, className: "decide-col", render: decisionCell },
          ].filter(Boolean),
        }).node
      : el("p", { class: "muted", text: "Nothing here needs a decision." });

    const counts = Object.entries(noAction).filter(([, count]) => count);
    const listed = el("div", { class: "rows tight", hidden: "" });
    const toggle = el("button", {
      class: "button secondary small",
      type: "button",
      text: "Show files",
      "aria-expanded": "false",
      onclick: () => {
        const open = listed.hidden;
        listed.hidden = !open;
        toggle.setAttribute("aria-expanded", String(open));
        setText(toggle, open ? "Hide files" : "Show files");
        if (open && !listed.firstChild) {
          const rows = held.flatMap((run) =>
            rowsOf(run.store)
              .filter((row) => NO_ACTION.has(row.class) && !row.accepted)
              .map((row) => ({ ...row, store: run.store })),
          );
          fill(
            listed,
            rows.length
              ? dataTable({
                  className: "w-scan-skipped",
                  caption: "Files not indexed that need no decision",
                  rows,
                  columns: [
                    { key: "path", label: "File", value: (row) => rootRel(row.path, row.store), render: (row) => pathCell(rootRel(row.path, row.store)) },
                    { key: "class", label: "Kind", value: (row) => row.class, render: (row) => el("span", { class: `pill class-${row.class}`, text: CLASS_LABELS[row.class] || row.class }) },
                    { key: "rule", label: "Why", sortable: false, render: (row) => lineCell(row.rule) },
                  ],
                }).node
              : el("p", { class: "muted", text: "The list is written when the run ends; the counts above are the scan's." }),
          );
        }
      },
    });
    const skipped = counts.length
      ? el(
          "div",
          { class: "scan-skipped" },
          el("span", {
            class: "meta",
            text: `${counts.map(([cls, count]) => `${n(count)} ${CLASS_LABELS[cls] || cls}`).join(" · ")} — not indexed, no action needed`,
          }),
          toggle,
        )
      : null;

    fill(
      node,
      el(
        "div",
        { class: "card-head" },
        el("h2", { text: held.length === 1 ? `Scanned ${held[0].store}` : `Scanned ${n(held.length)} folders` }),
        el("span", { class: "spacer" }),
        el("span", {
          class: "mono-chip",
          text: `scanned in ${spellScan(held.reduce((sum, run) => sum + (run.plan.seconds || 0), 0))} · ready to index`,
        }),
      ),
      summary,
      perStore,
      el("h3", { class: "eyebrow", text: items.length ? `Needs your decision · ${n(items.length)}` : "Needs your decision" }),
      decisions,
      skipped,
      listed,
      el("p", {
        class: "note",
        text: "Start indexing embeds the plan. A file left undecided stays refused, and any decision can be undone later on Files ▸ Decisions. Credential files are never offered.",
      }),
    );
  }

  return { node, paint, held: () => (state.runs?.runs || []).filter((run) => run.status === "review") };
}

async function indexView() {
  await refreshStores();
  const first = await refreshRuns();

  // Beside Start indexing, in the button row, rather than on a line of its
  // own: a line reserved for an answer that is usually not there held a blank
  // row above the cards, and emptying it pulled every card up a line at the
  // moment a run finished.
  const note = el("span", { class: "note index-note" });
  const cards = el("div", { class: "cards" });
  const queueList = el("div", { class: "queue" });
  /** Waiting runs' rows, by run id. */
  const queued = new Map();
  const queueCard = el(
    "div",
    { class: "card pad", hidden: true },
    el("span", { class: "eyebrow", text: "Waiting" }),
    el("p", {
      class: "subtitle",
      text: "In the order they will start. Each one begins by itself the moment a run finishes, whether or not this page is open.",
    }),
    queueList,
  );
  const settingsCard = el("div", { class: "card pad", hidden: true });
  const urlCard = el("div", { class: "card pad", hidden: true });
  const urlNote = el("div", { class: "note" });
  /* One placeholder, whatever has happened. It used to read "link to a page, a
   * PDF or a file" on the first render and "URL to fetch and index" after a
   * failed attempt, which reads as the field having changed its mind. */
  const urlField = el("input", { type: "text", placeholder: "link to a page, a PDF or a file" });
  /* The URL panel's own target store. The control the fetch actually read was
   * the "each folder becomes its own store" dropdown in a different control
   * group above, whose default is not a valid target for a URL — so every URL
   * was refused with a store-selection error that never said where to pick
   * one, and the private-address refusal was unreachable from this page. */
  const urlTarget = el("select", { class: "select" });
  const clearDone = el("button", {
    class: "button secondary small",
    type: "button",
    text: "Remove all finished",
    hidden: true,
  });
  /* Whether the note is holding an answer to a button that time can falsify —
   * "3 runs queued." stayed on screen long after the three had finished. */
  let transient = false;

  /* One card per run, kept across repaints so a card's log and its scroll
   * position survive the run's next poll.
   *
   * Keyed by the run's id rather than by its store. A second run against the
   * same store used to find the first one's card and write its header over it
   * while the body kept the previous run's log, so the card described two
   * different runs at once. */
  const drawn = new Map();
  /* Where each run's log has been read to. A cursor rather than an offset, so
   * two tabs reading the same run each see every line exactly once. */
  const cursors = new Map();

  function complain(message, focus) {
    note.className = "note bad";
    note.textContent = message;
    if (focus) focus.focus();
  }

  function say(message) {
    note.className = "note";
    note.textContent = message;
  }

  async function control(store, action, run, extra) {
    // A card's own control answers on its own card, under its buttons. On the
    // page's note the answer appeared above every card and pushed them all
    // down a line — the refusal a Pause meets when its run finishes under it.
    const card = drawn.get(run);
    const was = card && card.last().status;
    try {
      const answer = await post("/api/index/control", { store, action, run, ...(extra || {}) });
      if (card) {
        setText(card.problem, "");
        /* The route answers with the state it was asked for at once —
         * `pausing`, `running`, `stopping` — and the engine reaches it at its
         * next batch. The card says so now rather than at the next poll, and
         * a poll already in flight, asked before the click, is not painted
         * over it. Only if no poll has moved the card since the click: one
         * that already reads "paused" is further on than the answer. */
        if (answer && answer.state && card.last().status === was && TICKING.has(was)) {
          supersedeRuns();
          card.absorb({ ...card.last(), status: answer.state });
        }
      }
    } catch (e) {
      if (card) setText(card.problem, e.message);
      else complain(e.message);
    }
    await refreshRuns();
  }

  /** Pull whatever log lines a run has produced since this card last looked. */
  async function catchUpLog(run, card) {
    const after = cursors.get(run.id);
    // A run whose ring has already scrolled past this cursor — a tab away for
    // longer than five hundred lines — restarts from the oldest line still
    // held rather than silently showing a gap as continuity.
    const from = after === undefined ? run.log_from : after;
    let data;
    try {
      data = await api(
        `/api/index/log?store=${encodeURIComponent(run.store)}&run=${run.id}${
          from === undefined || from === null ? "" : `&after=${from}`
        }`,
      );
    } catch (_) {
      return;
    }
    for (const line of data.lines || []) logLine(card.log, line);
    if (data.cursor !== null && data.cursor !== undefined) cursors.set(run.id, data.cursor);
  }

  function paint(data) {
    const runs = data?.runs || [];
    paintStart(runs);
    const seen = new Set();
    for (const run of runs) {
      seen.add(run.id);
      let card = drawn.get(run.id);
      if (!card) {
        card = runCard(run, {
          hold: (wantPause) => control(run.store, wantPause ? "pause" : "resume", run.id),
          stop: (queued) => {
            if (queued) return control(run.store, "dequeue", run.id);
            /* "Also delete the store". Ticked for a store this run is
             * creating — it held nothing before, or the run has not started
             * and nobody knows yet — because stopping that run leaves an
             * empty store nobody asked for. Unticked for a store that held
             * files, and the dialog says how many stay. */
            const before = (drawn.get(run.id)?.last() || run).files_before;
            const box = el("input", {
              type: "checkbox",
              class: "pick",
              checked: !before,
            });
            const consequence = el("p", { class: "note" });
            const files = `${n(before)} file${before === 1 ? "" : "s"}`;
            const explain = () =>
              setText(
                consequence,
                box.checked
                  ? `${before ? `This store holds ${files}. ` : ""}The store is deleted once the run is undone. The files on disk are untouched.`
                  : before
                    ? `This store holds ${files}; they stay.`
                    : "The store stays, empty.",
              );
            box.addEventListener("change", explain);
            explain();
            ask({
              title: `Stop ${run.store}'s index run?`,
              body: "Everything it has embedded so far is undone, so the store is left exactly as it was before the run started. The other runs are untouched.",
              extra: [
                el("label", { class: "filters" }, box, el("span", { class: "subtitle", text: "Also delete the store" })),
                consequence,
              ],
              confirm: "Stop and undo",
              tone: "bad",
              run: () => control(run.store, "stop", run.id, { delete: box.checked }),
            });
          },
          // Dismissing a card, which touches nothing in the store. No confirm,
          // for the same reason taking a folder out of the queue has none.
          remove: () => control(run.store, "remove", run.id),
          // The card has just been unfolded and wants the lines it skipped.
          reveal: () => {
            const card = drawn.get(run.id);
            if (card) catchUpLog(run, card);
          },
        });
        // Not appended here: `arrange` below inserts it where the order puts
        // it, so a new card is one insertion rather than an append and a move.
        drawn.set(run.id, card);
      } else {
        card.absorb(run);
      }
      // Only what the reader can see. A finished card starts folded, and
      // fetching the log of every one of them on the first paint is a request
      // per card for lines nobody is looking at.
      if (card.isOpen()) catchUpLog(run, card);
    }
    // A run the daemon has forgotten — its card was removed, its store was
    // deleted, or the daemon restarted — loses its card rather than keeping a
    // stale one.
    for (const [id, card] of drawn) {
      if (seen.has(id)) continue;
      card.node.remove();
      drawn.delete(id);
      cursors.delete(id);
    }

    /* Live runs above finished ones, and newest first within each half. The
     * run somebody is watching used to be appended below every card that had
     * already finished, which on the fourth run put it off the bottom of the
     * screen. */
    const order = runs
      .slice()
      .sort((a, b) => {
        const live = Number(TICKING.has(b.status)) - Number(TICKING.has(a.status));
        return live || (b.id || 0) - (a.id || 0);
      })
      .map((run) => drawn.get(run.id)?.node)
      .filter(Boolean);
    // Only what is out of place moves. Every card used to be re-appended once
    // a second, which blurred the button under the reader's keyboard and
    // threw every log back to its top.
    arrange(cards, order);

    // "N runs queued." is an answer to a button, and it stopped being true the
    // moment the last run finished. It sits in the button row, so its going
    // moves nothing under it.
    if (transient && !runs.some((run) => TICKING.has(run.status)) && !(data?.queue || []).length) {
      transient = false;
      say("");
    }

    clearDone.hidden = !runs.some((run) => !TICKING.has(run.status));

    /* A page with nothing on it says nothing.
     *
     * Before the split this page ended in three cards about the corpus, so an
     * idle machine still had something under the button band. Those moved to
     * `Inside the index`, and what was left on a machine with no run in flight
     * was a heading, a text box and four buttons over half a screen of white.
     * The design draws the run card in both states; this is its idle one. */
    idle.hidden = runs.length > 0 || (data?.queue || []).length > 0;

    /* The queue, one row per waiting run, kept across polls like the cards.
     * It was rebuilt whole on every poll while anything waited, so the "Take
     * out of the queue" button under the pointer was a different button each
     * second and lost its focus with the one before it. A row's store and
     * paths are fixed for its run; only its place in line moves. */
    const queue = data?.queue || [];
    queueCard.hidden = !queue.length;
    const waiting = new Set();
    for (const row of queue) {
      const key = String(row.run ?? row.store);
      waiting.add(key);
      let drawnRow = queued.get(key);
      if (!drawnRow) {
        const position = el("span", { class: "key" });
        drawnRow = {
          position,
          node: el(
            "div",
            { class: "queue-row" },
            position,
            el("span", { class: "name", text: row.store }),
            pathCell((row.paths || []).join(", ")),
            el("span", { class: "spacer" }),
            el("button", {
              class: "button secondary small",
              type: "button",
              text: "Take out of the queue",
              // Nothing of it was embedded, so there is nothing to undo and
              // nothing to confirm.
              onclick: () => control(row.store, "dequeue", row.run),
            }),
          ),
        };
        queued.set(key, drawnRow);
      }
      setText(drawnRow.position, `${row.position}`);
    }
    for (const [key, row] of queued) {
      if (waiting.has(key)) continue;
      row.node.remove();
      queued.delete(key);
    }
    arrange(
      queueList,
      queue.map((row) => queued.get(String(row.run ?? row.store)).node),
    );

    paintSettings(data?.limits);
  }

  async function saveSetting(key, value) {
    // The answer is said inside the card, under the three fields. On the
    // page's note it appeared above everything, pushing the card that had just
    // been typed into down a line at the moment it was being looked at.
    try {
      const answer = await post("/api/index/settings", { [key]: value });
      settingsNote.className = "note";
      // What the daemon is running with now, in its words: all three take
      // effect at once, the running runs at their next batch.
      setText(settingsNote, answer.applied ? `Saved — ${answer.applied}.` : "Saved.");
      paintSettings(answer.limits);
    } catch (e) {
      settingsNote.className = "note bad";
      setText(settingsNote, e.message);
    }
  }

  /* The machine-limits card, built on the first reading and patched in place
   * after it.
   *
   * It used to be rebuilt with `fill` whenever any field of any limit moved,
   * and one of them always did: two reasons quoted the memory free now, and
   * the memory ceiling is derived from it, so the card was torn down and
   * rebuilt about once a second — under the cursor of anyone typing a number
   * into it, and with a new input in place of the focused one. Now a changed
   * number is a changed text node and nothing else. */
  const settingsNote = el("div", { class: "note" });
  const accel = accelSection();
  let limitsCard = null;
  function paintSettings(limits) {
    if (!limits) return;
    const machine = limits.machine || {};
    if (!limitsCard) {
      const cores = document.createTextNode("");
      const total = document.createTextNode("");
      const free = document.createTextNode("");
      limitsCard = {
        cores,
        total,
        free,
        fields: [
          settingField("runs_at_once", "runs at once", limits.runs_at_once, saveSetting),
          settingField("embed_threads", "threads each", limits.embed_threads, saveSetting),
          settingField("index_memory_mb", "MiB per store", limits.index_memory_mb, saveSetting),
        ],
      };
      fill(
        settingsCard,
        el("span", { class: "eyebrow", text: "How hard this machine may work" }),
        el(
          "p",
          { class: "subtitle" },
          cores,
          " logical core(s), ",
          total,
          " MiB of memory, ",
          free,
          " MiB free now. Each value below starts from that and is yours to change — they answer each other, so raising one moves what the others suggest. Each stops where this machine does.",
        ),
        el(
          "div",
          { class: "settings" },
          limitsCard.fields.map((field) => field.node),
        ),
        settingsNote,
        accel.node,
      );
    }
    // Every field of every limit, every time, and each patch is a no-op when
    // nothing moved. `embed_threads` is derived from the runs actually in
    // force, so changing `runs at once` changes the *sentence* under `threads
    // each` while leaving its number alone — which is why the three are
    // patched together rather than only the one that was saved.
    setText(limitsCard.cores, `${machine.logical_cores}`);
    setText(limitsCard.total, n(machine.total_memory_mb));
    setText(limitsCard.free, n(machine.available_memory_mb));
    const [runsAtOnce, threads, memory] = limitsCard.fields;
    runsAtOnce.update(limits.runs_at_once);
    threads.update(limits.embed_threads);
    memory.update(limits.index_memory_mb);
  }

  // The path field takes one path per line, so several folders can be started
  // without the picker at all.
  const field = el("textarea", {
    rows: "2",
    placeholder: "~/work/api\n~/work/cli",
    value: state.pendingPath || "",
  });
  state.pendingPath = "";

  const target = el(
    "select",
    { class: "select", "aria-label": "Where to index into" },
    el("option", { value: "each", text: "each folder becomes its own store" }),
    liveStores().map((store) =>
      el("option", { value: store.name, text: `add to ${store.name}` }),
    ),
  );

  /* Which stores could hold the paths in the box.
   *
   * A store is about its roots, and indexing a folder into a store whose roots
   * do not cover it is how the `semlith` store came to hold 262 files
   * belonging to `ultraship`. The daemon refuses it now; this is so the
   * dropdown does not offer it in the first place. */
  function paintTargets() {
    const paths = field.value
      .split("\n")
      .map((path) => path.trim())
      .filter(Boolean);
    const covers = (store) =>
      !paths.length ||
      paths.every((path) =>
        (store.roots || []).some((root) => root.present && path.startsWith(root.path)),
      );
    const chosen = target.value;
    fill(
      target,
      el("option", { value: "each", text: "each folder becomes its own store" }),
      liveStores().map((store) =>
        el("option", {
          value: store.name,
          text: covers(store)
            ? `add to ${store.name}`
            : `add to ${store.name} — outside its roots`,
          disabled: !covers(store),
        }),
      ),
    );
    // A selection that has just become invalid falls back to the choice that
    // is always right: a folder of its own.
    target.value = [...target.options].some((o) => o.value === chosen && !o.disabled)
      ? chosen
      : "each";
  }
  field.addEventListener("input", paintTargets);
  paintTargets();

  fill(
    urlTarget,
    liveStores().map((store) => el("option", { value: store.name, text: store.name })),
  );
  // With one store there is no choice to make, so the panel makes it. With
  // several the field starts empty and the button says so rather than the
  // request being refused by a control on another panel.
  const only = liveStores();
  urlTarget.value = only.length === 1 ? only[0].name : "";
  if (only.length !== 1) {
    urlTarget.prepend(el("option", { value: "", text: "choose a store…" }));
    urlTarget.value = "";
  }

  const picker = folderPicker({
    multiple: true,
    onChoose: (paths) => {
      if (!paths || !paths.length) return;
      const already = field.value.split("\n").map((p) => p.trim()).filter(Boolean);
      field.value = [...new Set([...already, ...paths])].join("\n");
      field.rows = Math.min(8, Math.max(2, field.value.split("\n").length));
    },
    onError: (message) => complain(message),
  });

  const projects = projectsChecklist({
    onChoose: (paths) => {
      field.value = paths.join("\n");
      field.rows = Math.min(8, Math.max(2, paths.length));
      target.value = "each";
    },
    onError: (message) => complain(message),
  });

  const folderButton = el(
    "button",
    {
      class: "button secondary",
      type: "button",
      "aria-pressed": "false",
      onclick: () => reveal("picker"),
    },
    icon(ICONS.folder),
    "Choose folders…",
  );
  const projectsButton = el(
    "button",
    {
      class: "button secondary",
      type: "button",
      "aria-pressed": "false",
      onclick: () => reveal("projects"),
    },
    icon(ICONS.folder),
    "Projects under a folder…",
  );
  const urlButton = el(
    "button",
    {
      class: "button secondary",
      type: "button",
      "aria-pressed": "false",
      onclick: () => reveal("url"),
    },
    icon(ICONS.file),
    "Add from a URL",
  );
  /* The machine's three numbers are a panel like the others rather than a card
   * standing open under the page. They are read once, changed rarely, and
   * having them permanently on screen gave the most static thing here the most
   * room. */
  const settingsButton = el(
    "button",
    {
      class: "button secondary",
      type: "button",
      "aria-pressed": "false",
      onclick: () => reveal("settings"),
    },
    icon(ICONS.sliders),
    "Machine limits",
  );

  /* The four ways to open something here are mutually exclusive: two open at
   * once is two answers to one question. */
  function reveal(which) {
    const wantPicker = which === "picker" && !picker.isOpen();
    const wantProjects = which === "projects" && !projects.isOpen();
    const wantUrl = which === "url" && urlCard.hidden;
    const wantSettings = which === "settings" && settingsCard.hidden;
    if (!wantPicker) picker.close();
    if (!wantProjects) projects.close();
    urlCard.hidden = !wantUrl;
    settingsCard.hidden = !wantSettings;
    if (wantPicker) picker.open("");
    if (wantProjects) projects.open("");
    if (wantUrl) urlField.focus();
    // "Projects under a folder…" makes each project its own store, so an
    // "add to <store>" sitting beside it is an instruction that contradicts
    // the picker that is open.
    if (wantProjects) target.value = "each";
    target.disabled = wantProjects;
    target.title = wantProjects
      ? "Each project under the folder becomes its own store."
      : "";
    folderButton.setAttribute("aria-pressed", String(wantPicker));
    projectsButton.setAttribute("aria-pressed", String(wantProjects));
    urlButton.setAttribute("aria-pressed", String(wantUrl));
    settingsButton.setAttribute("aria-pressed", String(wantSettings));
  }

  const idle = el(
    "div",
    { class: "card pad run-idle" },
    el(
      "div",
      { class: "card-head" },
      el("h2", { text: "Nothing is being read right now" }),
      el("span", { class: "spacer" }),
      el("span", { class: "mono-chip", text: "queued · writer idle" }),
    ),
    el("p", {
      class: "subtitle",
      text: "Name a folder above and press Scan: the plan comes first, then Start indexing. The run lives in the daemon, so you can close this tab and come back to it.",
    }),
    el("p", {
      class: "note",
      text: "Checkpointed every 30 seconds: vectors are written before the files they cover are marked indexed, so a run that is killed resumes rather than starting again.",
    }),
  );

  /* One button for the whole flow (2.7): it reads Scan, and a scan holds
   * its runs after the scan phase; while any run is held it reads Start
   * indexing, which queues them. A clean scan waits for the press too, so
   * nothing is ever embedded before its plan has been on screen. */
  const start = el("button", { class: "button", type: "button", text: "Scan" });
  const discard = el("button", {
    class: "button secondary",
    type: "button",
    text: "Discard scan",
    hidden: "",
  });
  const scan = scanPanel();
  let scanning = false;
  function paintStart(runs) {
    const held = (runs || []).filter((run) => run.status === "review");
    setText(start, scanning ? "Scanning…" : held.length ? "Start indexing" : "Scan");
    start.disabled = scanning;
    discard.hidden = !held.length || scanning;
    scan.paint(runs);
  }
  const addButton = el("button", {
    // The accent shape, like `Start indexing` beside it. Both of them begin
    // work on the machine, and one of the two reading as a quiet secondary
    // made the URL card look like a thing that had not been finished.
    class: "button",
    type: "button",
    text: "Fetch and index",
  });

  /** Ask the daemon to start the runs, and let the poll draw them. */
  async function begin(route, body) {
    start.disabled = true;
    try {
      const answer = await post(route, body);
      const started = (answer.runs || []).filter((run) => run.run !== undefined);
      const refused = (answer.runs || []).filter((run) => run.error);
      transient = true;
      const many = started.length === 1 ? "" : "s";
      const done =
        body.review === "always"
          ? `Scanned ${started.length} folder${many}: the plan is below. Press Start indexing when it looks right`
          : `${started.length} run${many} queued`;
      say(`${done}${refused.length ? `; ${refused.length} refused` : ""}.`);
      if (refused.length) {
        complain(refused.map((run) => `${run.path}: ${run.error}`).join("; "));
      }
      // Straight away rather than on the next tick: the cards are the answer
      // to the button, and a second of nothing reads as a button that failed.
      await refreshStores();
      await refreshRuns();
      return null;
    } catch (e) {
      complain(e.message);
      return e.message;
    } finally {
      start.disabled = false;
    }
  }

  start.addEventListener("click", async () => {
    const held = scan.held();
    if (held.length) {
      start.disabled = true;
      try {
        for (const run of held) {
          await post("/api/index/control", { store: run.store, run: run.id, action: "start" });
        }
        transient = true;
        say(`${held.length} run${held.length === 1 ? "" : "s"} queued.`);
      } catch (e) {
        complain(e.message);
      }
      await refreshRuns();
      paintStart(state.runs?.runs);
      return;
    }
    const paths = field.value
      .split("\n")
      .map((path) => path.trim())
      .filter(Boolean);
    if (!paths.length) {
      complain("Give a path to scan, or choose a folder.", field);
      return;
    }
    scanning = true;
    paintStart(state.runs?.runs);
    await begin("/api/index", { path: paths, store: target.value, review: "always" });
    scanning = false;
    paintStart(state.runs?.runs);
  });

  discard.addEventListener("click", async () => {
    for (const run of scan.held()) {
      // A folder that had no store before its scan leaves none behind.
      await control(run.store, "stop", run.id, run.files_before === 0 ? { delete: true } : {});
      await control(run.store, "remove", run.id);
    }
    say("Scan discarded.");
    await refreshStores();
    await refreshRuns();
    paintStart(state.runs?.runs);
  });

  addButton.addEventListener("click", async () => {
    const url = urlField.value.trim();
    if (!url) {
      urlNote.className = "note bad";
      urlNote.textContent = "Give an https URL to fetch.";
      urlField.focus();
      return;
    }
    urlNote.className = "note";
    urlNote.textContent = "";
    if (!urlTarget.value) {
      urlNote.className = "note bad";
      urlNote.textContent =
        "Pick the store to fetch into, in the selector beside this field.";
      urlTarget.focus();
      return;
    }
    const failed = await begin("/api/add", { url, store: urlTarget.value });
    // A failed fetch leaves the card open with what went wrong on it. Closing
    // it would take the message away and leave the page looking as though
    // nothing had been asked for.
    if (failed) {
      urlNote.className = "note bad";
      urlNote.textContent = failed;
      urlCard.hidden = false;
    }
  });

  clearDone.addEventListener("click", async () => {
    const finished = (state.runs?.runs || []).filter((run) => !TICKING.has(run.status));
    /* A card whose stop deleted its store has no store left to clear, and
     * `clear` naming one was refused after it had already dropped every such
     * card — so the refusal landed on the page note over the cards. Those are
     * removed one by one, by run, which the route answers without a store. */
    const gone = (run) => (run.delete || "").startsWith("the store was deleted");
    for (const run of finished.filter(gone)) await control(run.store, "remove", run.id);
    const stores = new Set(finished.filter((run) => !gone(run)).map((run) => run.store));
    for (const store of stores) await control(store, "clear");
  });

  paint(first);
  // The page's only subscription. The shared poll decides when; this decides
  // what with. No timer of this page's own.
  // The poll hands this the runs it just read, and `paint` reads them off it.
  // Calling it with nothing — which an added second painter made easy to do —
  // redraws the page as though every run had ended.
  state.onRuns = (data) => paint(data);
  // The lanes with the runs: a lane starts, downloads and carries its share
  // while a run is going, which is when the runs domain moves.
  accel.refresh();
  watchLive(["runs", "stores"], () => {
    refreshRuns();
    accel.refresh();
  });
  // A store deleted, or made, while the page is open: the dropdown stops
  // offering the one that is gone. Not while it is open under the pointer.
  state.onStores = () => {
    if (document.activeElement !== target) paintTargets();
  };

  return el(
    "div",
    { class: "view" },
    pageHead("Index", "What to read, and the run that reads it."),
    says(
      "Each folder becomes its own store, indexed by its own writer. A run lives in the daemon, not in this page — leaving, refreshing or closing the tab changes nothing, and a run ends only on its Stop or when ",
      mono("semlith start"),
      " does.",
    ),
    el(
      "div",
      { class: "field tall full area" },
      el("span", { class: "prefix", text: "paths" }),
      labelled("index-path", "Paths to index, one per line", field),
    ),
    /* Two groups on one line: the ways to choose what to read on the left,
     * and where it goes and the button that reads it on the right, kept
     * together when the line wraps. */
    el(
      "div",
      { class: "index-bar" },
      el("div", { class: "index-bar-group" }, folderButton, projectsButton, urlButton, settingsButton),
      el("div", { class: "index-bar-group end" }, target, discard, start),
    ),
    note,
    el(
      "div",
      { class: "scroller" },
      /* The design's order.
       *
       * This page was the design's two merged — `INDEX`, which is the picked
       * files and the run, and `CORPUS`, which is what the store already
       * holds. They are two pages again: everything about what the store
       * already contains moved to `Inside the index`, and what is left here is
       * the question "read this" and the answer "reading it". */
      /* Every panel the button band opens, in one slot directly under it.
       *
       * They were in four places: the folder picker here, the repository
       * checklist and the URL card below the corpus cards, and the queue and
       * the machine limits at the very foot of the page. Pressing `Choose
       * folders…` opened something you were looking at; pressing any of the
       * other three opened something a screen and a half away, with nothing
       * saying it had happened. One slot, so a button and what it opens are
       * always in the same relationship. */
      picker.node,
      projects.node,
      fill(
        urlCard,
        el("span", { class: "eyebrow", text: "Add from a URL" }),
        el("p", {
          class: "subtitle",
          text: "One https request, for exactly this URL — a page, a PDF, or a file on GitHub. Nothing is crawled and no credential is ever sent. The file is saved inside this store's downloads folder, never in your working tree.",
        }),
        el(
          "div",
          { class: "field tall full" },
          el("span", { class: "prefix", text: "url" }),
          labelled("index-url", "URL to fetch and index", urlField),
        ),
        el(
          "div",
          { class: "filters" },
          labelled("index-url-store", "Fetch into", urlTarget),
          el("span", { class: "spacer" }),
          addButton,
        ),
        urlNote,
      ),
      /* What the scan found, first: it is waiting on the person reading. */
      scan.node,
      queueCard,
      settingsCard,
      /* Then the run itself. */
      cards,
      idle,
      el("div", { class: "filters" }, el("span", { class: "spacer" }), clearDone),
    ),
  );
}

/* Inside the index: what the store already holds, measured.
 *
 * The design's CORPUS page, which until now was the bottom half of the
 * indexing page. Every figure comes from `/api/corpus`, which counts them out
 * of the store when it is called — the page's own subtitle says "measured, not
 * estimated", and a cached number under that sentence would make the page a
 * liar about itself.
 *
 * What the design draws and this does not: PDF pages, slides, spreadsheet
 * cells and notebook cells as *counts of pages and cells*. The store records
 * the text it extracted, not how many pages it came off, so those are counted
 * as files here and the panel says `files`. Adding the real figures is a
 * column in `files` and a change to every extractor, which is a release of its
 * own rather than a page.
 */

/** Words a printed page holds, and words a reader gets through in a minute.
 *
 * Both are conventions rather than measurements, so they are written down
 * where the page can point at them: roughly 500 words on a page of A4 set at a
 * readable size, and 250 words a minute is the figure reading research has
 * settled on for prose. The word count they multiply is measured. */
const WORDS_PER_PAGE = 500;
const WORDS_PER_MINUTE = 250;

/** `26 days`, `4 hours`, `18 minutes` — one unit, the largest that fits. */
function spellDuration(minutes) {
  if (minutes < 90) return `${Math.max(1, Math.round(minutes))} minutes`;
  const hours = minutes / 60;
  if (hours < 48) return `${Math.round(hours)} hours`;
  return `${Math.round(hours / 24)} days`;
}

/** `7 years, 6 months` between two unix seconds. */
function spellSpan(from, to) {
  if (!from || !to || to <= from) return "—";
  const months = Math.max(0, Math.round((to - from) / (86400 * 30.44)));
  const years = Math.floor(months / 12);
  const rest = months % 12;
  if (!years) return `${months} month${months === 1 ? "" : "s"}`;
  return `${years} year${years === 1 ? "" : "s"}${rest ? `, ${rest} month${rest === 1 ? "" : "s"}` : ""}`;
}

/** A label over a value, as the design's four small panels list their facts. */
function factRow(label, value) {
  return el(
    "div",
    { class: "kv-row" },
    el("span", { class: "k", text: label }),
    el("span", { class: "v", text: value }),
  );
}

function factPanel(title, rows) {
  return el(
    "section",
    { class: "card pad kv-panel" },
    el("h2", { text: title }),
    el("div", { class: "kv-list" }, rows.filter(Boolean)),
  );
}

/** The hover card, in the design's shape: a title with its colour, then
 * label and value rows. Every card in the portal is built here — the canvases
 * and the elements that carry one alike — so they cannot drift apart.
 * `tone` is a class that colours the dot: `tone-N`, an edge tier, `blue` or
 * `accent`. */
function tipCard(title, tone, rows) {
  return el(
    "div",
    { class: "tip-card" },
    el(
      "div",
      { class: "tip-head" },
      el("span", { class: `legend-dot ${tone || "blue"}` }),
      el("span", { class: "name", text: title }),
    ),
    rows.length
      ? el(
          "div",
          { class: "tip-rows" },
          rows.map(([label, value]) =>
            el("div", { class: "tip-row" }, el("span", { class: "k", text: label }), el("span", { class: "v", text: String(value) })),
          ),
        )
      : null,
  );
}

/** An element's own card, carried as `data-tip-title`, `data-tip-tone` and a
 * JSON `data-tip-rows`, so it goes through the one delegated tooltip. */
function tipCardOf(target) {
  let rows = [];
  try {
    rows = JSON.parse(target.getAttribute("data-tip-rows") || "[]");
  } catch (_) {
    rows = [];
  }
  return tipCard(target.getAttribute("data-tip-title") || "", target.getAttribute("data-tip-tone"), rows);
}

/** The twelve months ending at the newest one the store holds.
 *
 * The design draws twelve bars. A store indexed this morning has one month in
 * it, and one bar in a full-width card is a chart that has failed rather than a
 * corpus that is young — so the months with nothing in them are drawn at zero
 * and the chart keeps its shape. */
function twelveMonths(months) {
  const known = [...months.entries()].sort((a, b) => a[0].localeCompare(b[0]));
  if (!known.length) return [];
  const [lastYear, lastMonth] = known[known.length - 1][0].split("-").map(Number);
  const out = [];
  for (let back = 11; back >= 0; back -= 1) {
    // `Date.UTC` so the arithmetic wraps the year for us rather than by hand.
    const at = new Date(Date.UTC(lastYear, lastMonth - 1 - back, 1));
    const key = `${at.getUTCFullYear()}-${String(at.getUTCMonth() + 1).padStart(2, "0")}`;
    out.push([key, months.get(key) || 0]);
  }
  return out;
}

const MONTH_NAMES = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

function monthLabel(key) {
  const [year, month] = key.split("-").map(Number);
  return `${MONTH_NAMES[month - 1]} ${year}`;
}

/** Chunks added per month, full width, as the design draws it. */
function monthsCard(months, span) {
  const bars = twelveMonths(months);
  const peak = Math.max(1, ...bars.map(([, count]) => count));
  const total = bars.reduce((sum, [, count]) => sum + count, 0) || 1;
  return el(
    "section",
    { class: "card pad months-card" },
    el(
      "div",
      { class: "card-head" },
      el("h2", { text: "Chunks added per month" }),
      el("span", { class: "spacer" }),
      el("span", { class: "mono-chip", text: span }),
    ),
    bars.length
      ? el(
          "div",
          { class: "month-chart" },
          bars.map(([key, count], i) =>
            el(
              "div",
              {
                class: `month-col${i === bars.length - 1 ? " now" : ""}${count ? "" : " none"}`,
                tabindex: "0",
                "data-tip-title": monthLabel(key),
                "data-tip-tone": i === bars.length - 1 ? "accent" : "blue",
                "data-tip-rows": JSON.stringify([
                  ["chunks added", n(count)],
                  ["share of year", share(count, total)],
                  ["vs. peak", share(count, peak)],
                ]),
              },
              // Height as a share of the busiest month, and opacity with it, so
              // a quiet month reads as quiet rather than only as short. A month
              // with nothing in it still draws a sliver, because an absent bar
              // and a bar at zero say different things.
              sized("height", count ? Math.max(0.06, count / peak) : 0.05, { class: "col" }),
            ),
          ),
        )
      : el("div", { class: "rail-hint", text: "No files indexed yet." }),
    bars.length
      ? el(
          "div",
          { class: "month-axis" },
          bars.map(([key], i) =>
            el("span", {
              // Five labels across twelve bars, as the design spaces them.
              text: i % 3 === 0 || i === bars.length - 1 ? MONTH_NAMES[Number(key.split("-")[1]) - 1] : "",
            }),
          ),
        )
      : null,
    el("p", { class: "note", text: "When semlith read the file, not when it was written." }),
  );
}

/* The four confidences an edge can carry, in the order the graph rail lists
 * them, so the bar and the legend read the same way round everywhere. */
const EDGE_TIERS = ["extracted", "resolved", "ambiguous", "unresolved"];

/** Graph health, full width, as the design draws it. */
function graphHealthCard(corpus) {
  const tiers = new Map(corpus.tiers);
  const total = EDGE_TIERS.reduce((sum, tier) => sum + (tiers.get(tier) || 0), 0);
  const settled = (tiers.get("extracted") || 0) + (tiers.get("resolved") || 0);
  // The share the README quotes is over the edges that could be settled at
  // all: an unresolved edge names something no store here holds, and counting
  // it against the resolver is counting a dependency nobody indexed.
  const answerable = settled + (tiers.get("ambiguous") || 0);
  return el(
    "section",
    { class: "card pad health-card-full" },
    el(
      "div",
      { class: "card-head" },
      el("h2", { text: "Graph health" }),
      el("span", { class: "mono-chip", text: "counted from the edge table, every open store" }),
    ),
    el("span", { class: "eyebrow", text: "Call edges by tier" }),
    total
      ? el(
          "div",
          { class: "tier-bar", role: "img", "aria-label": EDGE_TIERS.map((t) => `${t} ${tiers.get(t) || 0}`).join(", ") },
          EDGE_TIERS.map((tier) =>
            // A tier with nothing in it still draws a sliver, because a legend
            // that names four and a bar that shows three is a bar with a
            // missing piece nobody can find.
            sized("width", Math.max(0.01, (tiers.get(tier) || 0) / total), {
              class: `seg ${tier}`,
              tabindex: "0",
              "data-tip-title": tier,
              "data-tip-tone": tier,
              "data-tip-rows": JSON.stringify([
                ["edges", n(tiers.get(tier) || 0)],
                ["share", share(tiers.get(tier) || 0, total)],
                ["of total", n(total)],
              ]),
            }),
          ),
        )
      : el("div", { class: "rail-hint", text: "No call edges yet." }),
    el(
      "div",
      { class: "tier-legend" },
      EDGE_TIERS.map((tier) =>
        el(
          "span",
          { class: "tier-key" },
          el("span", { class: `legend-dot ${tier}` }),
          el("span", { text: `${tier} ${n(tiers.get(tier) || 0)}` }),
        ),
      ),
    ),
    el("p", {
      class: "note",
      // The design's caption is a note about its own mock. This is the same
      // sentence about this store: what settled, and what the hints are for.
      text: total
        ? `${share(settled, answerable || 1)} of the answerable edges settled on one definition. Unresolved ones name code no open store holds — the standard library, a dependency nobody indexed, a typo.`
        : "Index something with a language that carries edges and this fills in.",
    }),
    el(
      "div",
      { class: "health-columns" },
      el(
        "div",
        { class: "health-col" },
        el("span", { class: "eyebrow", text: "Unresolved targets" }),
        el("span", {
          class: "health-figure",
          text: total ? `${n(corpus.unresolved)} · ${share(corpus.unresolved, total)}` : "—",
        }),
        corpus.unresolvedNames.length
          ? el(
              "div",
              { class: "chips" },
              corpus.unresolvedNames.slice(0, 7).map(([name, count]) =>
                el("span", {
                  class: "chip-flat",
                  tabindex: "0",
                  "data-tip-title": name,
                  "data-tip-rows": JSON.stringify([["calls", n(count)]]),
                  text: name,
                }),
              ),
            )
          : null,
        el("p", { class: "note", text: "Calls into code the store does not hold; hidden from views by default." }),
      ),
      el(
        "div",
        { class: "health-col" },
        el("span", { class: "eyebrow", text: "Names with several definitions" }),
        el("span", { class: "health-figure", text: n(corpus.ambiguousNames) }),
        el(
          "div",
          { class: "kv-list" },
          corpus.ambiguousWorst.length
            ? corpus.ambiguousWorst.slice(0, 5).map(([name, count]) => factRow(name, n(count)))
            : factRow("None", "—"),
        ),
      ),
      el(
        "div",
        { class: "health-col" },
        el("span", { class: "eyebrow", text: "Languages carrying edges" }),
        el("span", {
          class: "health-figure",
          text: `${n(corpus.languagesWithEdges)} of ${n(corpus.languageCount)}`,
        }),
        el("button", {
          class: "link-button",
          type: "button",
          text: "The language table",
          onclick: () => go("about"),
        }),
      ),
    ),
  );
}

async function corpusView() {
  await refreshStores();
  let data;
  try {
    data = await api("/api/corpus");
  } catch (e) {
    return el("div", { class: "view" }, pageHead("Inside the index"), error(e.message));
  }
  // The ledger's own summary, for the last of the three cards at the foot. It
  // is the one figure on this page that is about what the corpus *saved*
  // rather than about what it contains, and the ledger is where that is
  // measured.
  let ledger = null;
  try {
    ledger = await api("/api/ledger");
  } catch (_) {
    /* the card says so */
  }

  const stores = (data.stores || []).filter((row) => !row.error);
  const broken = (data.stores || []).filter((row) => row.error);
  const sum = (field) => stores.reduce((total, row) => total + (row[field] || 0), 0);

  const files = sum("files");
  const chunks = sum("chunks");
  const lines = sum("lines");
  const words = sum("words");
  const characters = sum("characters");
  const blank = sum("blank_lines");
  const comments = sum("comment_lines");
  const symbols = sum("symbols");
  const dim = stores.length ? stores[0].vector_dim || 384 : 384;
  const numbers = chunks * dim;

  // Merged across stores, because the reader has several open and the page is
  // about what this machine holds.
  const pile = (field, key) => {
    const into = new Map();
    for (const store of stores) {
      for (const row of store[field] || []) {
        into.set(row[key], (into.get(row[key]) || 0) + (row.count || row.lines || 0));
      }
    }
    return [...into.entries()].sort((a, b) => b[1] - a[1]);
  };
  const languages = pile("languages", "language");
  const kinds = pile("kinds", "name");
  const ambiguousWorst = pile("ambiguous_worst", "name");
  const languageTotal = languages.reduce((total, [, count]) => total + count, 0) || 1;

  const longest = stores
    .map((row) => row.longest_file)
    .filter(Boolean)
    .sort((a, b) => b.lines - a.lines)[0];
  const deepest = Math.max(0, ...stores.map((row) => row.deepest_path || 0));
  const first = Math.min(...stores.map((row) => row.first_indexed || 0).filter(Boolean));
  const last = Math.max(0, ...stores.map((row) => row.last_indexed || 0));
  const ms = stores.map((row) => row.median_query_ms || 0).filter(Boolean);
  const median = ms.length ? Math.round(ms.reduce((a, b) => a + b, 0) / ms.length) : 0;

  const months = new Map();
  for (const store of stores) {
    for (const row of store.months || []) {
      months.set(row.month, (months.get(row.month) || 0) + row.chunks);
    }
  }
  const busiest = [...months.entries()].sort((a, b) => b[1] - a[1])[0];

  const empty = !files;
  const pages = Math.round(words / WORDS_PER_PAGE);

  /* The language mix, by line rather than by file.
   *
   * By file is the wrong denominator for this question: a repository of four
   * hundred small TypeScript files and thirty large Rust ones is mostly Rust
   * by every measure that matters to a reader, and mostly TypeScript by file
   * count. The design measures lines, and this measures lines.
   *
   * `.mix-row .meter` deliberately, which is the shape the rest of the portal
   * draws a proportion in and the shape the drive checks for a CSP-dropped
   * width. */
  const mixCard = el(
    "section",
    { class: "card pad mix-card" },
    el(
      "div",
      { class: "card-head" },
      el("h2", { text: "Language mix, by line" }),
      el("span", { class: "spacer" }),
      el("span", {
        class: "mono-chip",
        text: `${n(lines)} lines · ${languages.length} language${languages.length === 1 ? "" : "s"}`,
      }),
    ),
    languages.length
      ? el(
          "div",
          { class: "mix-bar", role: "img", "aria-label": languages.map(([k, v]) => `${k} ${v} lines`).join(", ") },
          languages.map(([language, count], i) =>
            sized("width", count / languageTotal, {
              class: `seg tone-${i % 6}`,
              tabindex: "0",
              "data-tip-title": language,
              "data-tip-tone": `tone-${i % 6}`,
              "data-tip-rows": JSON.stringify([
                ["lines", n(count)],
                ["share", share(count, languageTotal)],
                ["of total", n(languageTotal)],
              ]),
            }),
          ),
        )
      : null,
    languages.length
      ? el(
          "div",
          { class: "mix" },
          languages.slice(0, 8).map(([language, count], i) =>
            el(
              "div",
              {
                class: "mix-row",
                tabindex: "0",
                "data-tip-title": language,
                "data-tip-tone": `tone-${i % 6}`,
                "data-tip-rows": JSON.stringify([
                  ["lines", n(count)],
                  ["share", share(count, languageTotal)],
                  ["of total", n(languageTotal)],
                ]),
              },
              el("span", { class: `swatch tone-${i % 6}` }),
              el("span", { class: "k", text: language }),
              el("span", { class: "meter" }, sized("width", count / languageTotal, { class: `tone-${i % 6}` })),
              el("span", { class: "v", text: n(count) }),
              el("span", { class: "pct", text: share(count, languageTotal) }),
            ),
          ),
        )
      : el("div", { class: "rail-hint", text: "Nothing indexed yet." }),
    el("p", { class: "note", text: "Every one of these carries graph edges as well as search." }),
  );

  const coverage = coveragePanel();

  const unresolved = stores.reduce((sum, row) => sum + (row.unresolved || 0), 0);
  const ambiguousNames = stores.reduce((sum, row) => sum + (row.ambiguous_names || 0), 0);
  const languagesWithEdges = stores.reduce((sum, row) => sum + (row.languages_with_edges || 0), 0);
  const tiers = pile("tiers", "name");
  const spanLabel = first && last
    ? `corpus spans ${new Date(first * 1000).toLocaleDateString(undefined, { month: "short", year: "numeric" })} → ${when(last)}`
    : "nothing indexed yet";

  const ratio = ledger && ledger.ratio ? ledger.ratio : 0;

  return el(
    "div",
    { class: "view" },
    pageHead(
      "Inside the index",
      "Measured from the store itself, not estimated. Nobody else can show you this, because nobody else keeps the whole corpus on your machine.",
      {
        pill: el("span", {
          class: "mono-chip",
          text: `${stores.length} store${stores.length === 1 ? "" : "s"} · recomputed on every read`,
        }),
      },
    ),
    // Two different failures, said differently. A store the fleet could not
    // open at all is the shared notice every cross-store page draws; a store
    // that opened and could not be measured is this page's own problem and is
    // named here with what it said.
    unreadableNotice(data.failed),
    broken.length
      ? el(
          "div",
          { class: "notice bad" },
          el("div", {
            class: "what",
            text: `${broken.length} store${broken.length === 1 ? "" : "s"} could not be measured, so ${
              broken.length === 1 ? "it is" : "they are"
            } not in these figures.`,
          }),
          el(
            "div",
            { class: "rows tight" },
            broken.map((row) =>
              el(
                "details",
                { class: "unreadable" },
                el("summary", {}, el("span", { class: "name", text: row.store })),
                el("pre", { class: "code", text: row.error }),
              ),
            ),
          ),
        )
      : null,
    empty
      ? el("p", { class: "subtitle", text: "Nothing is indexed yet, so there is nothing to measure." })
      : null,
    el(
      "div",
      { class: "stat-cards" },
      stat("Lines of code", n(lines), "counted, not estimated — comments and blanks separated out"),
      stat("Words indexed", n(words), "code, prose, slides, spreadsheets and notebooks together"),
      stat(
        "If it were printed",
        `${n(pages)} pp`,
        `a stack of A4 you can search in ${median ? `${median} ms` : "milliseconds"}`,
      ),
      stat(
        "Reading time",
        spellDuration(words / WORDS_PER_MINUTE),
        "non-stop at 250 words a minute, no sleep",
      ),
    ),
    /* The design's arrangement: two equal columns, the mix on the left and the
     * four panels as a two-by-two on the right, each card as tall as what it
     * holds. A row of four put the mix in a band of its own with a
     * quarter-width column of facts under each end of it. */
    el(
      "div",
      { class: "corpus-top" },
      mixCard,
      el(
        "div",
        { class: "corpus-panels" },
        factPanel(
          "What is in the prose",
          // Files, not pages and cells. The store keeps the text, not the page
          // it came off — said in the panel rather than in a comment nobody
          // reading the page can see.
          kinds.length
            ? kinds.map(([kind, count]) => factRow(kind, `${n(count)} file${count === 1 ? "" : "s"}`))
            : [factRow("Nothing indexed", "—")],
        ),
        factPanel("Shape of the code", [
          factRow("Average line", lines ? `${Math.round(characters / lines)} chars` : "—"),
          factRow("Comment lines", lines ? share(comments, lines) : "—"),
          factRow("Blank lines", lines ? share(blank, lines) : "—"),
          factRow("Longest file", longest ? `${shortPath(longest.path)} · ${n(longest.lines)}` : "—"),
          factRow("Deepest path", `${deepest} folder${deepest === 1 ? "" : "s"}`),
        ]),
        factPanel("Time in the corpus", [
          // Indexed, not written: the store records when it read a file and has
          // never been told when anybody wrote it.
          factRow("First read", Number.isFinite(first) && first ? new Date(first * 1000).toLocaleDateString() : "—"),
          factRow("Newest write", last ? when(last) : "—"),
          factRow("Span", spellSpan(first, last)),
          factRow("Busiest month", busiest ? `${busiest[0]} · ${n(busiest[1])} chunks` : "—"),
        ]),
        factPanel("The vectors themselves", [
          factRow("Vectors", n(chunks)),
          factRow("Numbers stored", n(numbers)),
          factRow("As float32 it would be", bytes(numbers * 4)),
          factRow("Quantised to int8", bytes(numbers)),
          factRow("Query at this size", median ? `${median} ms` : "not measured yet"),
        ]),
      ),
    ),
    monthsCard(months, spanLabel),
    graphHealthCard({
      tiers,
      unresolved,
      unresolvedNames: pile("unresolved_names", "name"),
      ambiguousNames,
      ambiguousWorst,
      languagesWithEdges,
      languageCount: languages.length,
    }),
    el(
      "div",
      { class: "fact-cards" },
      el(
        "section",
        { class: "fact-card" },
        el("h2", { text: `${n(characters)} characters` }),
        el("p", { text: "Every keystroke in the corpus, kept on this machine and nowhere else." }),
      ),
      el(
        "section",
        { class: "fact-card" },
        el("h2", { text: median ? `${median} ms` : "milliseconds" }),
        el("p", {
          text: median
            ? "The middle of every retrieval this machine has recorded — meaning, not just words."
            : "Ask this index something and the ledger will start recording how long it took.",
        }),
      ),
      el(
        "section",
        { class: "fact-card" },
        el("h2", { text: ratio ? `${ratio.toFixed(1)}× fewer tokens` : `${n(symbols)} definitions` }),
        el("p", {
          text: ratio
            ? `What your agents read versus reading those files whole · coverage ${ledger.coverage}% · ${ledger.tier}`
            : "Every definition the graph extracted from this corpus.",
        }),
      ),
    ),
    el("span", { class: "eyebrow", text: "What the graph covers" }),
    coverage.node,
  );
}

/** The repositories under a folder, as a checklist.
 *
 * A folder of projects is the case the folder picker handles badly: ticking
 * twelve repositories one directory at a time is twelve walks into and back
 * out of the same parent. This asks the daemon which of the children are
 * repositories and offers them all at once, already ticked.
 */
function projectsChecklist(options) {
  const { onChoose, onError } = options || {};
  const card = el("div", { class: "card picker", hidden: true });
  const ticked = new Set();

  async function open(path) {
    let data;
    try {
      data = await api(`/api/projects?path=${encodeURIComponent(path || "")}`);
    } catch (e) {
      if (onError) onError(e.message);
      return false;
    }
    card.hidden = false;
    const rows = data.projects || [];
    // Repositories start ticked because that is what was asked for; one
    // already indexed starts unticked but is shown, so the list says what is
    // covered rather than quietly omitting it.
    for (const row of rows) {
      if (data.repositories && !row.indexed && !ticked.size) ticked.add(row.path);
    }

    const paint = () => open(data.path);
    fill(
      card,
      el(
        "div",
        { class: "crumbs" },
        // The same navigation the folder picker beside it has had all along.
        // Without it this opened at the home directory and stayed there, so a
        // monorepo anywhere else on the machine was unreachable.
        el("button", {
          class: "button secondary small",
          type: "button",
          text: "Up",
          disabled: !data.parent,
          onclick: () => open(data.parent),
        }),
        el("span", { class: "where", text: data.path }),
        el("span", { class: "spacer" }),
        el("button", {
          class: "button secondary small",
          type: "button",
          text: "All",
          onclick: () => {
            for (const row of rows) ticked.add(row.path);
            paint();
          },
        }),
        el("button", {
          class: "button secondary small",
          type: "button",
          text: "None",
          onclick: () => {
            ticked.clear();
            paint();
          },
        }),
        el("button", {
          class: "button small",
          type: "button",
          disabled: !ticked.size,
          text: ticked.size ? `Use ${ticked.size}` : "Use",
          onclick: () => {
            card.hidden = true;
            if (onChoose) onChoose([...ticked]);
            ticked.clear();
          },
        }),
        el("button", {
          class: "button secondary small",
          type: "button",
          text: "Close",
          onclick: () => {
            card.hidden = true;
          },
        }),
      ),
      el("p", {
        class: "subtitle",
        text: data.repositories
          ? "The repositories directly under this folder. One level only: a monorepo is one store, and its nested repositories are its own business."
          : "Nothing under this folder is a repository, so these are its plain subfolders.",
      }),
      el(
        "div",
        { class: "entries" },
        rows.length
          ? rows.map((row) => {
              const box = el("input", {
                type: "checkbox",
                class: "tick",
                "aria-label": `Index ${row.name}`,
                checked: ticked.has(row.path),
                onchange: () => {
                  if (box.checked) ticked.add(row.path);
                  else ticked.delete(row.path);
                  paint();
                },
              });
              return el(
                "div",
                { class: "entry-row" },
                box,
                el(
                  "button",
                  {
                    class: "entry",
                    type: "button",
                    title: `Open ${row.path}`,
                    onclick: () => open(row.path),
                  },
                  icon(ICONS.folder),
                  el("span", { class: "name", text: row.name }),
                  row.indexed ? pill(`in ${row.indexed}`, "warn") : null,
                ),
              );
            })
          : empty("Nothing here that Semlith can index."),
      ),
      // Everything else under this folder, so the picker can be walked to
      // wherever the projects actually are.
      (data.folders || []).filter((f) => !rows.some((row) => row.path === f.path)).length
        ? el(
            "div",
            { class: "entries" },
            el("p", { class: "subtitle", text: "Or open one of these:" }),
            (data.folders || [])
              .filter((f) => !rows.some((row) => row.path === f.path))
              .map((folder) =>
                el(
                  "div",
                  { class: "entry-row" },
                  el(
                    "button",
                    {
                      class: "entry",
                      type: "button",
                      title: `Open ${folder.path}`,
                      onclick: () => open(folder.path),
                    },
                    icon(ICONS.folder),
                    el("span", { class: "name", text: folder.name }),
                  ),
                ),
              ),
          )
        : null,
    );
    return true;
  }

  return {
    node: card,
    open,
    close: () => {
      card.hidden = true;
    },
    isOpen: () => !card.hidden,
  };
}

// -------------------------------------------------------- install panel

/* What this machine has set up, and the one thing the page can fix itself.
 *
 * The install commands that used to head this card belonged on a machine that
 * does not have semlith yet — which is not the machine reading this page. What
 * is useful here is the state of each step and a way to act on the one that is
 * not done.
 */
function installPanel() {
  const card = el("div", { class: "card pad install-panel" });
  const title = () => el("span", { class: "card-title", text: "This machine" });
  fill(card, title(), el("p", { class: "subtitle", text: "Checking…" }));

  const stateWord = {
    done: "done",
    "already-done": "already done",
    skipped: "not done",
    failed: "failed",
  };
  const tone = (state) =>
    state === "done" || state === "already-done" ? "good" : state === "failed" ? "bad" : "warn";

  function paint(setup) {
    const note = el("div", { class: "note" });

    const fixPath = el("button", {
      class: "button small",
      type: "button",
      text: "Add it to PATH",
      onclick: async () => {
        fixPath.disabled = true;
        note.className = "note";
        note.textContent = "Editing your shell profile…";
        try {
          const done = await post("/api/setup", { step: "path" });
          note.textContent = `${done.step.detail} — open a new terminal for it to take effect.`;
          paint(done.status);
        } catch (e) {
          note.className = "note bad";
          note.textContent = e.message;
          fixPath.disabled = false;
        }
      },
    });

    const install = el("button", {
      class: "button small",
      type: "button",
      text: "Install it",
      disabled: true,
      onclick: async () => {
        install.disabled = true;
        check.disabled = true;
        note.className = "note";
        note.textContent = "Downloading and verifying…";
        try {
          const done = await post("/api/upgrade", { action: "apply" });
          note.textContent = done.restart;
        } catch (e) {
          note.className = "note bad";
          note.textContent = e.message;
        } finally {
          check.disabled = false;
        }
      },
    });

    const check = el("button", {
      class: "button secondary small",
      type: "button",
      text: "Check for updates",
      onclick: async () => {
        check.disabled = true;
        note.className = "note";
        note.textContent = "Asking GitHub…";
        try {
          const found = await post("/api/upgrade", { action: "check" });
          note.textContent = found.available
            ? `${found.latest} is available; you are on ${found.installed}.`
            : `Already on the newest build, ${found.installed}.`;
          if (found.blocked) note.textContent += ` ${found.blocked}`;
          install.disabled = !found.available || Boolean(found.blocked);
        } catch (e) {
          note.className = "note bad";
          note.textContent = e.message;
        } finally {
          check.disabled = false;
        }
      },
    });

    fill(
      card,
      title(),
      el(
        "div",
        { class: "rows" },
        (setup.steps || []).map((step) =>
          el(
            "div",
            { class: "kv step" },
            pill(stateWord[step.state] || step.state, tone(step.state)),
            el("span", { class: "card-title", text: step.name }),
            lineCell(step.detail, "meta"),
            // The one step this page can perform, and only while there is
            // something to do: the step's own state is the answer, not the
            // live PATH, which a daemon started before the edit never sees.
            // The others are an install and a model download, which belong to
            // the command that owns them.
            step.name === "path" && step.state !== "already-done" && step.state !== "done"
              ? el("span", { class: "spacer" })
              : null,
            step.name === "path" && step.state !== "already-done" && step.state !== "done"
              ? fixPath
              : null,
          ),
        ),
      ),
      note,
      el("hr", { class: "rule" }),
      el("span", { class: "eyebrow", text: `Installed version ${setup.version}` }),
      el("div", { class: "actions" }, check, install),
    );
  }

  api("/api/setup")
    .then(paint)
    .catch((e) => fill(card, title(), error(e.message)));

  return card;
}

// ---------------------------------------------------------------- agents

/* ---------------------------------------------------------------- doctor */

/* `semlith doctor`, as a page.
 *
 * The parity rule is why it exists: the command was added in 0.18.0, so its
 * view is in the same release. It reads the same two functions the command
 * prints — the per-client report and the four Privacy rules that are readings
 * of this machine — so the page and the terminal cannot disagree about whether
 * a client is registered or a rule holds.
 *
 * The repair buttons post to `/api/privacy/fix`, which calls what
 * `semlith doctor --fix` calls. A second implementation in the browser would
 * be a second answer to what "safe" means. */
async function doctorView() {
  let data;
  try {
    data = await api("/api/doctor");
  } catch (e) {
    return el("div", { class: "view" }, pageHead("Doctor"), error(e.message));
  }

  const state = (c) => {
    if (c.note) return { text: "cannot register", kind: null };
    if (c.registered) return { text: `registered (${c.scope || "user"})`, kind: "good" };
    if (!c.command) return { text: "not registered", kind: "bad" };
    if (!c.present) return { text: "not installed", kind: null };
    if (c.scope === "project")
      return { text: "one project only", kind: "bad" };
    return { text: "installed, not registered", kind: "bad" };
  };

  /* What this client has beyond its registration, in the words the terminal
   * uses. A client that documents none of the three says nothing rather than
   * three columns of "paste needed" on twenty rows that never asked for one. */
  const steering = (r) => {
    const parts = [];
    if (r.skill && r.skill !== "paste") parts.push(`skill ${r.skill === "present" ? "linked" : r.skill}`);
    if (r.hook && r.hook !== "paste") parts.push(`hook ${r.hook}${r.hook_mode ? ` (${r.hook_mode})` : ""}`);
    if (r.always_load !== undefined) parts.push(r.always_load ? "always loaded" : "alwaysLoad missing");
    if (r.explorer !== undefined) parts.push(r.explorer ? "research agent" : "no research agent");
    if (r.rules && r.rules !== "paste") parts.push(`rule ${r.rules}`);
    return parts;
  };

  const clients = dataTable({
    caption:
      "Every documented client, whether it is on this machine, whether it is registered, whether the skill and the steering hook are installed, and what would fix it.",
    rows: data.clients || [],
    columns: [
      { key: "name", label: "Client", sortable: true, value: (r) => r.name },
      {
        key: "state",
        label: "semlith",
        sortable: true,
        value: (r) => state(r).text,
        render: (r) => {
          const s = state(r);
          return pill(s.text, s.kind);
        },
      },
      {
        /* Registration says the client can reach semlith. Steering says its
         * agent will actually call it, which is a different question and the
         * one 0.24.0 exists to answer. Same source as the terminal's own line:
         * `semlith doctor` computes both and this renders what it computed. */
        key: "steering",
        label: "Skill & hook",
        sortable: true,
        value: (r) => steering(r).join(", "),
        render: (r) => {
          const parts = steering(r);
          if (!parts.length) return el("span", { class: "sub", text: "—" });
          return el(
            "span",
            { class: "pills" },
            ...parts.map((part) =>
              pill(part, part.endsWith("present") || part.endsWith("linked") ? "good" : "warn"),
            ),
          );
        },
      },
      {
        key: "repair",
        label: "To fix",
        render: (r) =>
          r.repair
            ? copyField(r.repair)
            : el("span", { class: "sub", text: r.note ? "nothing to run" : "—" }),
      },
    ],
  });

  const rulesBox = el("div", { class: "strip" });
  const note = el("div", { class: "note" });

  const paintRules = (rules) => {
    rulesBox.textContent = "";
    for (const rule of rules) {
      const row = el(
        "div",
        { class: "stat fact" },
        el("span", { class: "eyebrow", text: rule.id }),
        el("span", {
          /* Three states. A rule this platform cannot take a reading for —
           * the two mode rules on Windows — is neither green nor red, because
           * a tick it has not earned is worse than no tick. */
          class:
            rule.applicable === false
              ? "fact-value"
              : rule.ok
                ? "fact-value"
                : "fact-value bad",
          text: rule.applicable === false ? "not applicable" : rule.ok ? "holds" : "fails",
        }),
        el("span", { class: "sub", text: rule.check }),
      );
      if (!rule.ok && rule.manual) row.appendChild(copyField(rule.manual));
      /* A button only where a repair qualifies. A rule the daemon cannot
       * repair — `private addresses` is set in the environment the daemon
       * inherited, and no process can unset a variable in its parent's — gets
       * the manual step and nothing else, rather than a button that apologises
       * after the click. */
      if (!rule.ok && rule.repair) {
        const fix = el("button", {
          class: "button secondary small",
          type: "button",
          text: "Fix",
          onclick: async () => {
            fix.disabled = true;
            note.className = "note";
            note.textContent = "Applying…";
            try {
              const out = await post("/api/privacy/fix", { rule: rule.id });
              const applied = (out.applied || [])[0];
              /* What it changed and what it was before, so the user can undo
               * it by hand. The rules come back re-read, so the row redraws
               * from a measurement rather than from the click having worked. */
              note.textContent = applied
                ? `${applied.path}: ${applied.now}, was ${applied.was}` +
                  (applied.rechecked_ok ? "" : " — the rule still does not hold")
                : "Nothing to apply.";
              paintRules(out.rules || []);
            } catch (e) {
              note.className = "note bad";
              note.textContent = e.message;
              fix.disabled = false;
            }
          },
        });
        row.appendChild(fix);
      }
      rulesBox.appendChild(row);
    }
  };
  paintRules(data.rules || []);

  /* The GPU check: `semlith doctor --gpu`, from the page. Each lane embeds the
   * committed known-answer texts and is compared with the CPU's fp32 vectors,
   * so "the GPU works" is a cosine and a rate rather than a device name. On
   * request only: the first check of a lane downloads what it needs, which is
   * nothing a page load should do on its own. */
  const gpuNote = el("div", { class: "note" });
  const gpuTable = dataTable({
    caption:
      "Each accelerator lane's check: whether its vectors match the CPU's, on which device and variant, the lowest cosine over the known answers, and its rate.",
    rows: [],
    columns: [
      { key: "lane", label: "Lane", value: (r) => LANE_NAMES[r.lane] || r.lane },
      {
        key: "result",
        label: "Result",
        value: (r) => (r.reason ? "n/a" : r.passed ? "ok" : "FAIL"),
        render: (r) =>
          r.reason ? pill("n/a", null) : r.passed ? pill("ok", "good") : pill("FAIL", "bad"),
      },
      { key: "device", label: "Device", value: (r) => r.device || r.reason || "—" },
      { key: "variant", label: "Variant", value: (r) => r.variant || "—" },
      {
        key: "cosine",
        label: "Cosine",
        value: (r) => (typeof r.cosine === "number" ? r.cosine.toFixed(4) : "—"),
      },
      {
        key: "rate",
        label: "Chunks/s",
        value: (r) => (typeof r.chunks_per_s === "number" ? perSecond(r.chunks_per_s) : "—"),
      },
    ],
  });
  gpuTable.node.hidden = true;
  const gpuButton = el("button", {
    class: "button secondary small",
    type: "button",
    text: "Run the GPU check",
    onclick: async () => {
      gpuButton.disabled = true;
      gpuButton.setAttribute("aria-busy", "true");
      setText(gpuButton, "Checking…");
      gpuNote.className = "note";
      setText(gpuNote, "Checking each lane. The first check of a lane downloads what it needs, so it can take a minute.");
      try {
        const answer = await post("/api/doctor/gpu", {});
        gpuTable.update(answer.checks || []);
        gpuTable.node.hidden = false;
        setText(gpuNote, "");
      } catch (e) {
        gpuNote.className = "note bad";
        setText(gpuNote, e.message);
      } finally {
        gpuButton.disabled = false;
        gpuButton.removeAttribute("aria-busy");
        setText(gpuButton, "Run the GPU check");
      }
    },
  });

  // What the table is a list of, said once above it rather than counted off
  // the rows by the reader — the page is twenty-seven rows and two of them
  // matter.
  const rows = data.clients || [];
  const registered = rows.filter((c) => c.registered).length;
  const fixable = rows.filter((c) => !c.registered && c.repair).length;

  return el(
    "div",
    { class: "view" },
    pageHead(
      "Doctor",
      "Whether each agent client on this machine can reach semlith, and what to run for the ones that cannot.",
    ),
    /* The rules first, then the table.
     *
     * The clients table is twenty-seven rows and pages ten at a time, so the
     * rules underneath it were below a screenful of table on every visit —
     * and they are the part that answers "is this machine set up correctly",
     * which is the question the page is for. The table is the detail.
     *
     * The rules are already cards — one `.stat` each — so they sit in the view
     * beside their heading rather than inside a second card. A card of cards
     * is a border drawn around some borders. */
    el(
      "div",
      { class: "head" },
      el("span", { class: "card-title", text: "Rules" }),
      el("span", {
        class: "meta",
        text: "what the daemon found when it looked, not what the documentation says",
      }),
    ),
    rulesBox,
    el(
      "div",
      { class: "card pad" },
      el(
        "div",
        { class: "head" },
        el("span", { class: "card-title", text: "GPU check" }),
        el("span", { class: "spacer" }),
        gpuButton,
      ),
      el("p", {
        class: "subtitle",
        text: "Whether each accelerator lane's vectors agree with the CPU's, and how fast it runs. The same check as semlith doctor --gpu.",
      }),
      gpuNote,
      gpuTable.node,
    ),
    el(
      "div",
      // `card pad`, like every other section on every other page. A bare card
      // has no padding, so the title sat against the border and the table
      // squared off the corner under it.
      { class: "card pad" },
      el(
        "div",
        { class: "head" },
        el("span", { class: "card-title", text: "Clients" }),
        el("span", { class: "spacer" }),
        el("span", {
          class: "meta",
          text: `${n(registered)} registered · ${n(fixable)} to fix · ${n(rows.length)} known`,
        }),
      ),
      clients.node,
    ),
    note,
  );
}

async function agentsView() {
  let data;
  try {
    data = await api("/api/agents");
  } catch (e) {
    return el("div", { class: "view" }, pageHead("Agents"), error(e.message));
  }

  noteAgents(data);
  const clients = data.clients || [];
  const tools = data.tools || [];
  const live = data.connections || [];
  const endpoint = data.endpoint || { url: "", open: false };

  // ---- the endpoint, in the page header
  const address = el("code", { class: "text", text: endpoint.url });
  const endpointNote = el("div", { class: "note" });
  const toggle = el("button", {
    class: "button secondary small",
    type: "button",
    text: endpoint.open ? "Stop" : "Start",
    onclick: () => {
      // Every other consequential action on this page confirms — delete a
      // store, forget a file, stop a run — and this is the one that takes
      // semlith away from every connected agent at once. Starting it back up
      // costs nothing, so only the stop asks.
      if (!endpoint.open) return apply();
      ask({
        title: "Close the MCP endpoint?",
        body: "Every connected agent loses semlith until it is started again. The daemon, the watcher and the portal are unaffected, and Start puts it back.",
        confirm: "Close the endpoint",
        tone: "bad",
        run: apply,
      });
    },
  });

  /** Open or close the endpoint, once whoever asked has confirmed. */
  async function apply() {
    toggle.disabled = true;
    endpointNote.className = "note";
    endpointNote.textContent = endpoint.open ? "Closing…" : "Opening…";
    try {
      const done = await post("/api/endpoint", { open: !endpoint.open });
      endpoint.open = done.open;
      toggle.textContent = done.open ? "Stop" : "Start";
      endpointNote.textContent = done.open
        ? "The endpoint is answering. A configured client reconnects on its next call."
        : "The endpoint is closed. The daemon, the watcher and the portal are unaffected.";
      fill(statePill, el("i", {}), done.open ? "answering" : "closed");
      statePill.className = done.open ? "pill good" : "pill warn";
    } catch (e) {
      endpointNote.className = "note bad";
      endpointNote.textContent = e.message;
    } finally {
      toggle.disabled = false;
    }
  }
  const statePill = pill(endpoint.open ? "answering" : "closed", endpoint.open ? "good" : "warn");

  // ---- rotation
  const keyNote = el("div", { class: "note" });
  const rotate = el("button", {
    class: "button secondary small",
    type: "button",
    text: "Rotate key",
    onclick: async () => {
      rotate.disabled = true;
      keyNote.className = "note";
      keyNote.textContent = "Rotating…";
      try {
        const done = await post("/api/key", {});
        // Only if the page was already showing the real one; otherwise the
        // stanza still names the variable, which is now correct for the new
        // key without anybody touching it.
        if (stanzas.revealed) stanzas.key = done.key;
        showClient(chosen);
        const carried = done.updated || [];
        // Said plainly, because the two halves have different consequences:
        // what was rewritten needs nothing done to it, and what was not needs
        // the stanza above pasted in before the old key stops working.
        const rewrote = carried.length
          ? `Carried the new key into ${carried.length} configuration file${
              carried.length === 1 ? "" : "s"
            }: ${carried.join(", ")}.`
          : "No configuration file on this machine carried the old key.";
        keyNote.textContent = done.previous_valid
          ? `New key. ${rewrote} The previous one keeps working for fifteen minutes, so a session already open finishes — any client configured elsewhere needs the new stanza before then.`
          : `New key. ${rewrote} The previous one is refused now; any client configured elsewhere needs the new stanza.`;
      } catch (e) {
        keyNote.className = "note bad";
        keyNote.textContent = e.message;
      } finally {
        rotate.disabled = false;
      }
    },
  });

  // ---- the key itself, only when asked for
  /* Masked until asked for. The route does not send a preview either: a
   * preview of an agent key still begins `sml_`, and the point is that nothing
   * about the credential arrives unasked. */
  const MASKED = data.key_set ? "sml_" + "•".repeat(24) : "no key yet";
  const keyBox = el("code", { class: "text", text: MASKED });
  const reveal = el("button", {
    class: "button secondary small",
    type: "button",
    text: "Reveal",
    onclick: async () => {
      if (stanzas.revealed) {
        // Pressed again: put it away. A page left open on a screen should not
        // keep showing a live credential because somebody looked at it once.
        stanzas.revealed = false;
        stanzas.key = "${" + KEY_ENV + "}";
        keyBox.textContent = MASKED;
        reveal.textContent = "Reveal";
        showClient(chosen);
        return;
      }
      reveal.disabled = true;
      try {
        const shown = await post("/api/agents/reveal", {});
        stanzas.revealed = true;
        stanzas.key = String(shown.key);
        keyBox.textContent = stanzas.key;
        reveal.textContent = "Hide";
        showClient(chosen);
      } catch (e) {
        keyNote.className = "note bad";
        keyNote.textContent = e.message;
      } finally {
        reveal.disabled = false;
      }
    },
  });

  // ---- connected clients
  const connected = dataTable({
    className: "w-agents",
    caption: "Every documented client, whether it is on this machine, and whether it is registered.",
    sort: "name",
    grow: false,
    rows: live,
    columns: [
      {
        key: "name",
        label: "Connected",
        value: (c) => c.name,
        render: (c) => el("div", { class: "who" }, el("span", { class: "dot good" }), c.name),
      },
      { key: "transport", label: "Transport", className: "meta", value: (c) => c.transport },
      { key: "revision", label: "Revision", className: "meta narrow-drop", value: (c) => c.revision },
      {
        key: "queries",
        label: "Queries",
        className: "num",
        value: (c) => c.queries,
        render: (c) => n(c.queries),
      },
    ],
  });

  // ---- stanzas, grouped
  const GROUPS = [
    ["terminal", "Terminal"],
    ["editor", "Editors"],
    ["desktop", "Desktop"],
  ];
  /* What a stanza carries.
   *
   * The variable form by default: a configuration file that names
   * `${SEMLITH_AGENT_KEY}` keeps working across every rotation, and a file that
   * carries the key itself goes stale the moment somebody presses Rotate. The
   * real key is fetched only when Reveal is pressed — `/api/agents` no longer
   * returns it, so a page that is merely open never receives the credential
   * that opens the MCP endpoint. */
  const KEY_ENV = data.key_env || "SEMLITH_AGENT_KEY";
  const stanzas = { key: "${" + KEY_ENV + "}", revealed: false };
  let group = GROUPS.find(([id]) => clients.some((c) => c.group === id))[0];
  let chosen = clients.findIndex((c) => c.group === group);
  const tabs = el("div", { class: "tabs" });
  const chips = el("div", { class: "filters" });
  const body = el("div", { class: "stanza" });

  /** The HTTP form, built here from the key the route just handed back. */
  /** The placeholder the README prints where a real key goes. */
  const KEY_SLOT = "${SEMLITH_AGENT_KEY}";

  /** Whether a stanza configures the endpoint rather than a subprocess. */
  const overHttp = (text) => text.includes("/mcp") || text.includes("--transport http");

  function httpStanza() {
    return `{\n  "mcpServers": {\n    "semlith": {\n      "url": "${endpoint.url}",\n      "headers": { "Authorization": "Bearer ${stanzas.key}" }\n    }\n  }\n}`;
  }

  function showGroup(id) {
    group = id;
    const first = clients.findIndex((c) => c.group === id);
    showClient(first < 0 ? chosen : first);
  }

  function showClient(index) {
    chosen = index;
    const client = clients[index];
    fill(
      tabs,
      GROUPS.map(([id, label]) => {
        const count = clients.filter((c) => c.group === id).length;
        return el(
          "button",
          {
            class: "tab",
            type: "button",
            "aria-pressed": String(id === group),
            onclick: () => showGroup(id),
          },
          label,
          el("span", { class: "count", text: String(count) }),
        );
      }),
    );
    fill(
      chips,
      clients.map((c, i) =>
        c.group === group
          ? el("button", {
              class: "chip",
              type: "button",
              "aria-pressed": String(i === chosen),
              text: c.name,
              onclick: () => showClient(i),
            })
          : null,
      ),
    );
    if (!client) {
      fill(body, empty("No client stanza is compiled into this build."));
      return;
    }
    // The documented stanzas. They name ${SEMLITH_AGENT_KEY}, which is what a
    // client should carry: it survives a rotation, and the key stays in one
    // file with one set of permissions. Pressing Reveal substitutes the literal
    // value for a reader who wants to paste it somewhere that cannot read an
    // environment variable.
    const own = (client.stanzas || []).map((s) => ({
      format: s.format,
      text: s.text.trimEnd().replaceAll(KEY_SLOT, stanzas.key),
    }));
    // A one-line command belongs in a field with a Copy beside it, not in a
    // slab of dark code: it is a thing you paste into a shell, not a file you
    // are going to read.
    // Shell one-liners, labelled by what they register rather than mixed in
    // with the block above: several clients configure the endpoint in a file
    // but can only add a subprocess from their CLI, and an unlabelled command
    // under an HTTP stanza reads as a second way to do the same thing.
    const shells = own.filter((s) => s.format === "sh");
    const wired = shells.filter((s) => overHttp(s.text));
    const spawned = shells.filter((s) => !overHttp(s.text));
    const blocks = own.filter((s) => s.format !== "sh");
    const http = blocks.filter((s) => overHttp(s.text));
    const stdio = blocks.filter((s) => !overHttp(s.text));
    const config = http.length ? http : stdio;
    fill(
      body,
      el(
        "div",
        { class: "head" },
        el("span", { class: "card-title", text: client.name }),
        el("span", { class: "meta", text: client.note }),
      ),
      // A client with no HTTP stanza of its own gets the generic one, which is
      // the shape most schemas take. A client with one gets its own: `httpUrl`
      // for Gemini, `serverUrl` for Windsurf, TOML for Codex — a generic block
      // beside those is a second, wrong answer.
      // One configuration block, not two. A client that can reach the
      // endpoint is shown the endpoint; one that cannot — Zed, Claude Desktop,
      // Amazon Q, which have nowhere to put a header — is shown the subprocess
      // it can run. Printing both left a reader choosing between two answers
      // with nothing to choose on.
      el("span", {
        class: "eyebrow",
        text: config.length && config === http
          ? "Over HTTP — one endpoint, one key, no subprocess"
          : "As a subprocess — no key needed",
      }),
      (config.length ? config.map((s) => s.text) : [httpStanza()]).map((text) =>
        codeBlock(text, "Copy stanza"),
      ),
      wired.length ? el("span", { class: "eyebrow", text: "Or from a terminal" }) : null,
      wired.map((s) => copyField(s.text, true)),
      spawned.length
        ? el("span", { class: "eyebrow", text: "Or add the subprocess from a terminal" })
        : null,
      spawned.map((s) => copyField(s.text, true)),
    );
  }
  showClient(chosen);

  /* `semlith setup --register-all`, as a control.
   *
   * Ten clients have no registration command of their own, so the only way
   * semlith reaches them is by writing their configuration file — and
   * `src/setup.rs` states the rule that bends: a tool that edits a file it does
   * not own eventually corrupts one. It holds for every default install. This
   * is the user overriding it for their own machine, which is why the plan is
   * fetched and shown first and nothing is written until a second click.
   *
   * Each file is backed up beside itself before its first write, each merge
   * keeps every key it did not come to change, and a file that does not parse
   * is refused rather than replaced. */
  const planBox = el("div", { class: "rules" });
  const planNote = el("div", { class: "note" });
  let planned = null;

  /* The safe one is the primary. The amber button that writes into ten real
   * configuration files across the machine used to be the primary and the dry
   * run beside it the secondary, so the visual hierarchy was upside down
   * against the risk and the writing button was reachable without ever having
   * looked at the preview. */
  const write = el("button", {
    class: "button secondary small",
    type: "button",
    text: "Write these files",
    hidden: true,
    onclick: async () => {
      write.disabled = true;
      planNote.className = "note";
      planNote.textContent = "Writing…";
      try {
        const out = await post("/api/agents/register", { confirm: true });
        const written = out.written || [];
        planNote.textContent = written.length
          ? `Wrote ${written.length} file${written.length === 1 ? "" : "s"}. Each has a .semlith-backup beside it.`
          : "Nothing needed writing.";
        planned = out.plan || [];
        paintPlan();
      } catch (e) {
        planNote.className = "note bad";
        planNote.textContent = e.message;
        write.disabled = false;
      }
    },
  });

  const paintPlan = () => {
    planBox.textContent = "";
    for (const item of planned || []) {
      const what =
        item.action === "create"
          ? "will be created"
          : item.action === "merge"
            ? "will be merged into"
            : item.action === "already-done"
              ? "already has semlith"
              : `will be left alone — ${item.reason}`;
      planBox.appendChild(
        el(
          "div",
          { class: "rule-row" },
          el(
            "div",
            { class: "rule-head" },
            el("span", { class: "rule-id", text: item.client }),
          ),
          el("code", { class: "rule-check", text: item.path }),
          el("span", { class: "sub", text: what }),
        ),
      );
    }
    const pending = (planned || []).some(
      (p) => p.action === "create" || p.action === "merge",
    );
    write.hidden = !pending;
    write.disabled = !pending;
  };

  const preview = el("button", {
    class: "button small",
    type: "button",
    text: "Show what would be written",
    onclick: async () => {
      preview.disabled = true;
      planNote.className = "note";
      planNote.textContent = "Reading…";
      try {
        const out = await post("/api/agents/register", {});
        planned = out.plan || [];
        planNote.textContent = planned.length
          ? "Nothing has been written yet."
          : "There is no client on this machine whose file semlith would write.";
        paintPlan();
      } catch (e) {
        planNote.className = "note bad";
        planNote.textContent = e.message;
      } finally {
        preview.disabled = false;
      }
    },
  });

  const registerAll = el(
    "div",
    { class: "card pad" },
    el("span", { class: "card-title", text: "Register the clients that have no command" }),
    // Inline code as code, not as a pair of backtick characters. Every other
    // code reference on this page is styled; this one was printed verbatim.
    says(
      "Ten of the twenty-seven cannot be asked to register themselves, so semlith would write their configuration file. Every path is listed before anything is written, each file is backed up beside itself, and one that does not parse is left alone. This is the terminal's ",
      mono("semlith setup --register-all"),
      ".",
    ),
    el("div", { class: "head" }, preview, write),
    planBox,
    planNote,
  );

  // ---- the login service
  /* Whether the endpoint survives a reboot.
   *
   * The header above says "One endpoint, every client, no per-client process",
   * and until 0.21.0 that endpoint existed only for as long as somebody kept a
   * terminal open for it. This card is where that sentence is either true on
   * this machine or not, named in the mechanism the platform actually uses.
   *
   * There is no Install button. No route installs a login service, and a
   * button that posts to nothing is worse than the command it would hide. */
  const MECHANISMS = {
    launchd: "a launchd login agent",
    systemd: "a systemd user service",
    schtasks: "a Windows logon task",
  };

  /** A path or a command, labelled, monospace, and copyable. */
  const labelled = (label, value) =>
    el(
      "div",
      { class: "rows tight" },
      el("span", { class: "eyebrow", text: label }),
      copyField(value),
    );

  const serviceCard = () => {
    // An older daemon, or a page cached from one, sends no `service` at all.
    // No card is better than a card reporting "not installed" because it was
    // never told either way.
    if (!data.service) return null;
    const status = data.service.status || {};
    const mechanism = MECHANISMS[status.mechanism] || "a login service";
    // Nothing at all when the daemon has not recorded a start: "never started"
    // under a daemon that is answering this very request is a sentence that
    // reads as a bug in the page.
    const started = data.service.last_started
      ? ` Last started ${when(data.service.last_started)}.`
      : "";

    const bits = [];
    if (status.installed) {
      bits.push(
        says(
          `The daemon is installed as ${mechanism}, so the endpoint is answering again after a reboot without anybody opening a terminal for it.${started}`,
        ),
      );
    } else {
      bits.push(
        says(
          `The daemon is not installed as a login service, so the endpoint lives exactly as long as whatever started it: close that terminal, or reboot, and every client configured below loses semlith until somebody starts it again.${started}`,
        ),
        labelled("To install", "semlith start --service"),
      );
    }
    if (status.definition) bits.push(labelled("Definition", status.definition));
    if (status.log) bits.push(labelled("Log", status.log));
    // Said rather than left implied. A logon task is restarted when it *fails*;
    // nothing supervises one that exited cleanly. A page that prints
    // "installed" over both platforms claims a parity the product does not
    // have.
    if (status.installed && status.restarts === false) {
      bits.push(
        el("div", {
          class: "note",
          text: "A logon task is restarted only when it failed. One that exited cleanly stays stopped until the next logon, so a daemon stopped on purpose is a daemon started again on purpose.",
        }),
      );
    }

    return el(
      "div",
      { class: "card pad dense" },
      el(
        "div",
        { class: "head" },
        el("span", { class: "card-title", text: "Login service" }),
        el("span", { class: "spacer" }),
        pill(
          status.installed ? "installed" : "not installed",
          status.installed ? "good" : "warn",
        ),
      ),
      bits,
    );
  };

  // ---- the clients somebody on this machine actually has
  /* `semlith doctor`, cut to the clients in use.
   *
   * All twenty-seven rows are the Doctor page's job. The question here is
   * narrower — of the clients this machine has, is each one reaching the
   * endpoint — so the rows are the ones `doctor` itself calls in use and
   * nothing else. A client nobody has is not a finding.
   *
   * `disabled_here` is deliberately not a state on this page. "Here" for the
   * daemon is wherever a service manager started it, usually `/`, which is
   * nobody's working directory; a row reading "switched off here" would be
   * answering a question about a directory the reader has never stood in.
   * `disabled_in` is the honest form — it names directories, and the reader is
   * the one who knows which of them they work in. */
  const inUse = (data.doctor || []).filter((c) => c && c.in_use);

  const clientState = (c) => {
    if (c.note) return { text: "cannot register", kind: null };
    if (c.registered) return { text: `registered (${c.scope || "user"})`, kind: "good" };
    if (!c.command) return { text: "not registered", kind: "bad" };
    if (!c.present) return { text: "not installed", kind: null };
    if (c.scope === "project") return { text: "one project only", kind: "bad" };
    return { text: "installed, not registered", kind: "bad" };
  };

  /* Why a row is not green, and what to run about it.
   *
   * `explain` is written for `semlith doctor` run from a directory, so it says
   * "this directory" — true of the shell that ran the command, and not of a
   * daemon a launch agent started from the root of the disk. Where it says
   * that, the line is composed from `disabled_in` instead, which names the
   * directories rather than pointing at one. */
  const clientWhy = (c) => {
    const lines = [];
    if (c.note) lines.push(el("span", { class: "sub", text: c.note }));
    if (c.explain && !/this directory/i.test(c.explain))
      lines.push(el("span", { class: "sub", text: c.explain }));
    const off = (c.disabled_in || []).filter(Boolean);
    if (off.length)
      lines.push(el("span", { class: "meta", text: `switched off for: ${off.join(", ")}` }));
    if (c.repair) lines.push(copyField(c.repair));
    // A row the daemon calls a fault is coloured like one, so it does not get
    // to stay silent about why: a red pill over an em dash is an accusation
    // with no sentence after it. There is nothing to repair for a client that
    // is not here, so what it gets is the reading rather than a command.
    if (!lines.length && !c.present && c.fault)
      lines.push(
        el("span", {
          class: "sub",
          text: c.command
            ? `Nothing named ${c.command} is on this machine.`
            : "None of this client's configuration files are on this machine.",
        }),
      );
    return lines.length
      ? el("div", { class: "rows tight" }, lines)
      : el("span", { class: "sub", text: "—" });
  };

  const inUseTable = inUse.length
    ? dataTable({
        caption:
          "Every agent client installed on this machine, whether it is registered with semlith, and what to run for the ones that are not.",
        sort: "name",
        grow: false,
        rows: inUse,
        columns: [
          { key: "name", label: "Client", sortable: true, value: (c) => c.name },
          {
            key: "state",
            label: "semlith",
            sortable: true,
            value: (c) => clientState(c).text,
            // `fault` is the daemon's own verdict, so a row it calls a fault
            // reads as one whatever the text works out to. A client with a
            // note is not a fault and must not borrow the colour of one.
            render: (c) => {
              const s = clientState(c);
              return pill(s.text, c.fault ? "bad" : s.kind);
            },
          },
          // Not `narrow-drop`: the reason is the point of the row, so on a
          // phone the table scrolls sideways inside its card — the Doctor
          // page's behaviour with the same rows — rather than hiding the one
          // column that says what to do. The page itself never scrolls.
          { key: "why", label: "Why", className: "why", render: clientWhy },
        ],
      })
    : null;

  const inUseCard = data.doctor
    ? el(
        "div",
        { class: "card pad" },
        el(
          "div",
          { class: "head" },
          el("span", { class: "card-title", text: "Clients in use" }),
          el("span", { class: "spacer" }),
          el("span", {
            class: "meta",
            text: `${n(inUse.length)} of ${n((data.doctor || []).length)} known`,
          }),
        ),
        inUseTable
          ? inUseTable.node
          : empty("No documented client is installed on this machine."),
        says("All twenty-seven, including the ones nobody here has, are on the Doctor page."),
      )
    : null;

  return el(
    "div",
    { class: "view" },
    pageHead("Agents", "One endpoint, every client, no per-client process.", {
      pill: statePill,
      actions: [
        el("div", { class: "copyfield worded" }, address, copyButton(endpoint.url, "Copy", true)),
        toggle,
        rotate,
      ],
    }),
    endpointNote,
    el(
      "div",
      { class: "grid scroller agent-row" },
      el(
        "div",
        { class: "rows" },
        live.length
          ? connected.node
          : el(
              "div",
              { class: "card pad dense" },
              el("span", { class: "card-title", text: "Connected" }),
              empty("No client is talking to this daemon right now."),
            ),
        el(
          "div",
          { class: "card pad dense tools" },
          el(
            "div",
            { class: "head" },
            el("span", { class: "card-title", text: "Tools exposed" }),
            el("span", {
              class: "meta",
              text: `${tools.length} tool${tools.length === 1 ? "" : "s"}`,
            }),
          ),
          // The schema is the first thing every agent reads and the last thing
          // anyone thinks to measure: it is paid once per session, before a
          // single question is asked. Measured from what this daemon is
          // serving right now rather than quoted from a release, so the number
          // cannot go stale on the page.
          el(
            "div",
            { class: "cost" },
            el("span", { class: "card-title", text: "What the tool list costs" }),
            el("span", {
              class: "cost-line",
              /* Measured, not "about". The daemon counts it with a store's own
               * tokenizer where one is loaded and says which counter produced
               * the figure, the same two tiers the ledger reports — because
               * four characters to a token is an estimate and this page should
               * not present one as a measurement. */
              text: `${tools.length} tools · ${n(data.tool_list_bytes || 0)} bytes · ${n(
                data.tool_list_tokens || Math.ceil((data.tool_list_bytes || 0) / 4),
              )} tokens per session`,
            }),
            el("span", {
              class: "cost-note",
              text: `read once, before the agent asks anything — counted ${
                data.tool_list_tier || "chars4"
              }`,
            }),
          ),
          el(
            "div",
            { class: "tool-grid" },
            // The name and what it is for, both read from the tool's own
            // definition: a second copy is how a tool ends up served with one
            // description and documented with another.
            tools.flatMap((tool) => [
              el("span", { class: "tool-name", text: tool.name }),
              el("span", { class: "tool-about", text: tool.about }),
            ]),
          ),
        ),
        // Under the tools rather than beside the registration card: it is a
        // short card about this machine, and paired with a wide one it was
        // stretched to that card's height and centred in it.
        installPanel(),
      ),
      el("div", { class: "card pad stanzas" }, tabs, chips, body),
    ),
    /* Everything below is this project's own, after the four panels the v4
     * design draws. They were interleaved with them — the service card, the
     * agent key and the two registration cards sat between the endpoint note
     * and `Connected`, and the installer sat inside the design's own grid
     * between `Tools exposed` and the stanzas — so a reader following the
     * design's argument met four unrelated cards in the middle of it. */
    /* Two columns, equal height. These were five full-width cards down a page
     * that is mostly narrow text, so the page scrolled for a screenful of
     * content and every card was the width of the window for no reason. The
     * service card and the key are a pair — what runs the daemon and what an
     * agent authenticates with — and the two registration cards are another. */
    el(
      "div",
      { class: "grid agent-row agent-extras" },
      serviceCard(),
      el(
      "div",
      { class: "card pad dense" },
      el("span", { class: "card-title", text: "Agent key" }),
      el("div", { class: "copyfield" }, keyBox, el("div", { class: "actions" }, reveal)),
      says(
        "Shown truncated, and fetched in full only when you press Reveal — the page does not receive it just for being open. No registration semlith writes carries it: a registered client launches ",
        mono("semlith mcp"),
        ", which reads the key from ",
        mono(data.key_path || "~/.semlith/agent.key"),
        " itself, so a rotation reconfigures nothing. The key is for the HTTP stanzas below, which a daemon on another machine still needs — paste one of those and you export ",
        mono("${" + KEY_ENV + "}"),
        " yourself.",
      ),
    ),
    ),
    keyNote,
    registerAll,
    inUseCard,
  );
}

// --------------------------------------------------------------- privacy

async function privacyView() {
  let data;
  try {
    data = await api("/api/privacy");
  } catch (e) {
    return el("div", { class: "view" }, pageHead("Privacy"), error(e.message));
  }

  // Live: a repair applied here or from `semlith doctor --fix` moves the
  // privacy counter, so the rows say what is true now rather than what was
  // true when the page was opened.
  watchLive(["privacy"], repaintView);

  const tokenBox = el("code", { class: "text", text: data.token_preview || "—" });
  const rotateNote = el("div", { class: "note" });
  const rotate = el("button", {
    class: "button secondary small",
    type: "button",
    text: "Rotate",
    onclick: async () => {
      rotate.disabled = true;
      rotateNote.className = "note";
      rotateNote.textContent = "Rotating…";
      try {
        const fresh = await post("/api/rotate", {});
        // The rotate response is the one place the whole token appears after
        // the printed URL, and this page has to take the new one before its
        // next request: nothing else will tell it. What is shown is still the
        // preview — a page that prints a live credential in full is a page
        // someone screenshots.
        session.set(String(fresh.token));
        tokenBox.textContent = `${String(fresh.token).slice(0, 16)}…`;
        rotateNote.textContent =
          "Rotated. The old token stopped working immediately. This is the portal's own session token, not the agent key: no client stanza carries it, so nothing needs reconfiguring. The agent key is rotated on the Agents page.";
      } catch (e) {
        rotateNote.className = "note bad";
        rotateNote.textContent = e.message;
      } finally {
        rotate.disabled = false;
      }
    },
  });

  const fact = (key, value, why) =>
    el(
      "div",
      { class: "stat fact" },
      el("span", { class: "eyebrow", text: key }),
      el("span", { class: "fact-value", text: value }),
      el("span", { class: "sub", text: why }),
    );

  /* The rules, with a way to act on them. Until 0.18.0 this page reported a
   * failing rule and left the user to work out the command; a page that tells
   * someone their store home is world-readable and stops there has done half a
   * job.
   *
   * Every failing row gets the manual step, always, because that half works
   * everywhere — including on Windows, where the mode rules have no reading at
   * all. A row gets a button only where the daemon can repair it safely:
   * narrowing, idempotent, on a path semlith owns, and re-checked by the rule's
   * own check afterwards. `private addresses` has no button, and cannot: it is
   * set in the environment the daemon inherited, and no process can unset a
   * variable in its parent's.
   *
   * The button posts to the engine `semlith doctor --fix` calls. A repair
   * implemented in the browser would be a second answer to what "safe" means,
   * on the page whose whole subject is that question. */
  const rulesBox = el("div", { class: "rules" });
  const fixNote = el("div", { class: "note" });

  const paintRules = (rules) => {
    rulesBox.textContent = "";
    for (const rule of rules) {
      const unmeasured = rule.applicable === false;
      const row = el(
        "div",
        { class: rule.ok || unmeasured ? "rule-row" : "rule-row bad" },
        el(
          "div",
          { class: "rule-head" },
          el("span", { class: "dot " + (unmeasured ? "" : rule.ok ? "good" : "warn") }),
          el("span", { class: "rule-id", text: rule.id }),
        ),
        el("p", { class: "rule-text", text: rule.rule }),
        // The reading, under the rule rather than beside it: it is a path or a
        // count often enough that a column would spend the whole card's width
        // on one of them and wrap the rest.
        el(
          "div",
          { class: "rule-found" },
          el("span", { class: "eyebrow", text: "found" }),
          el("code", { class: "rule-check", text: rule.check }),
        ),
      );
      if (!rule.ok && rule.manual) {
        row.appendChild(
          el(
            "div",
            { class: "rule-found" },
            el("span", { class: "eyebrow", text: "to fix" }),
            copyField(rule.manual),
          ),
        );
      }
      if (!rule.ok && rule.repair) {
        const fix = el("button", {
          class: "button secondary small",
          type: "button",
          text: "Fix",
          onclick: async () => {
            fix.disabled = true;
            fixNote.className = "note";
            fixNote.textContent = "Applying…";
            try {
              const out = await post("/api/privacy/fix", { rule: rule.id });
              const applied = (out.applied || [])[0];
              fixNote.textContent = applied
                ? `${applied.path}: ${applied.now}, was ${applied.was}` +
                  (applied.rechecked_ok ? "" : " — the rule still does not hold")
                : "Nothing to apply.";
              paintRules(out.rules || []);
            } catch (e) {
              fixNote.className = "note bad";
              fixNote.textContent = e.message;
              fix.disabled = false;
            }
          },
        });
        row.appendChild(fix);
      }
      rulesBox.appendChild(row);
    }
  };
  paintRules(data.rules || []);

  /* The scan, behind a button rather than in the page's load.
   *
   * It reads the stored text of every file in every store, which on a large one
   * takes long enough that loading it with the page would make the whole
   * Privacy page wait on a measurement most visits never look at. */
  const scanNote = el("div", { class: "note" });
  const scanBox = el("div", { class: "rows tight" });

  const scanTable = dataTable({
    caption: "Every file a store is still holding that semlith would refuse to index today.",
    sort: "path",
    columns: [
      { key: "store", label: "Store", className: "meta", sortable: true, value: (f) => f.store },
      {
        key: "path",
        label: "Path",
        sortable: true,
        value: (f) => f.path,
        render: (f) => pathCell(f.path),
      },
      /* The rule, or the kind of credential and the line it sits on — never
       * the text that matched. The route does not return it, and a page whose
       * subject is what stays on this machine would be a poor place to reprint
       * a secret in order to report that one was found. */
      {
        key: "why",
        label: "Why it would be refused",
        sortable: false,
        render: (f) => lineCell(f.why),
      },
      {
        key: "forget",
        label: "",
        sortable: false,
        render: (f) =>
          el("button", {
            class: "forget",
            type: "button",
            text: "Forget",
            onclick: () =>
              ask({
                title: "Forget this file?",
                body: `${f.path} — its chunks and vectors are dropped from ${f.store}. The file on disk is untouched.`,
                confirm: "Forget it",
                tone: "bad",
                run: () => forgetFound([f]),
              }),
          }),
      },
    ],
    rows: [],
  });

  async function forgetFound(findings) {
    scanNote.className = "note";
    scanNote.textContent = "";
    // One call per store: a write names the store it is for, and a set that
    // spans two of them is two writes rather than an ambiguous one.
    const byStore = new Map();
    for (const f of findings) {
      if (!byStore.has(f.store)) byStore.set(f.store, []);
      // The store's own key, not the plain path beside it. On Windows they
      // differ — the key carries the `\\?\` prefix — and a forget is a
      // lookup, so it has to be spelled the way the store spelled it. `path`
      // is what the row shows a person.
      byStore.get(f.store).push(f.key || f.path);
    }
    try {
      for (const [store, paths] of byStore) await post("/api/forget", { paths, store });
    } catch (e) {
      scanNote.className = "note bad";
      scanNote.textContent = e.message;
      // Rethrown so the dialog that asked stays open and shows it too.
      throw e;
    }
    // Scanned again rather than spliced: the list has to redraw from a fresh
    // reading, not from the assumption that the write did what it was asked.
    await runScan();
  }

  const scanButton = el("button", {
    class: "button secondary small",
    type: "button",
    text: "Scan",
    onclick: () => runScan(),
  });

  async function runScan() {
    scanButton.disabled = true;
    scanButton.textContent = "Scanning…";
    scanNote.className = "note";
    scanNote.textContent = "";
    try {
      const found = (await api("/api/privacy/scan")).findings || [];
      scanTable.update(found);
      const stores = [...new Set(found.map((f) => f.store))];
      fill(
        scanBox,
        found.length
          ? [
              el(
                "div",
                { class: "bulk" },
                el("span", {
                  class: "meta",
                  text: `${n(found.length)} file${found.length === 1 ? "" : "s"} would be refused today`,
                }),
                el("span", { class: "spacer" }),
                el("button", {
                  class: "button danger small",
                  type: "button",
                  text: "Forget all",
                  onclick: () =>
                    ask({
                      title: `Forget ${n(found.length)} file${found.length === 1 ? "" : "s"}?`,
                      body: `Their chunks and vectors are dropped from ${stores.join(
                        " and ",
                      )}. The files on disk are untouched.`,
                      confirm: `Forget ${n(found.length)} file${found.length === 1 ? "" : "s"}`,
                      tone: "bad",
                      run: () => forgetFound(found),
                    }),
                }),
              ),
              scanTable.node,
            ]
          : empty("Nothing this store holds would be refused today."),
      );
    } catch (e) {
      scanNote.className = "note bad";
      scanNote.textContent = e.message;
    } finally {
      scanButton.disabled = false;
      scanButton.textContent = "Scan again";
    }
  }

  /* The four steps, numbered, each with its own copy button. A packet capture
   * is the only one of them that proves anything on its own; the others are
   * what make the first one quick to believe. */
  const step = (number, what, command) =>
    el(
      "div",
      { class: "step-row" },
      el("span", { class: "n", text: number }),
      el(
        "div",
        { class: "what" },
        el("div", { class: "say", text: what }),
        copyField(command),
      ),
    );

  return el(
    "div",
    { class: "view" },
    pageHead(
      "Privacy",
      "The claim is “nothing leaves this machine”. This page is how you check it yourself, in about a minute.",
      { pill: pill(data.airgap ? "airgap mode armed" : "airgap off", data.airgap ? "good" : null) },
    ),
    el(
      "div",
      { class: "strip" },
      fact("Bind address", data.bind, "loopback only, with no flag to change it"),
      fact("CORS", "none", "no origin may read this server's responses"),
      fact("Host check", "localhost only", "a foreign Host header gets 400 before anything runs"),
      fact("Assets", "include_bytes!", "the page you are reading is inside the binary"),
      fact("Telemetry", "none", "no analytics, and no update check Semlith makes on its own"),
      // Said on this page, not only on the Ledger page. Somebody checking the
      // privacy claim should find the one thing semlith writes down about
      // them here, with how to stop it, rather than discovering it elsewhere.
      fact(
        "Retrieval ledger",
        "records locally",
        "the daemon prints this on every start; --no-ledger stops it for a session, SEMLITH_LEDGER=0 for a machine, and the rows never leave the store",
      ),
      fact("Model cache", data.model_cached ? "cached" : "not downloaded", data.model_cache),
      // Said here because this is where somebody comes to find out what
      // semlith reads. The binary contacts cloud.semlith.com only after a
      // `semlith cloud login`, which this release does not have — so the
      // honest reading is that nothing does.
      fact(
        "Cloud",
        "not connected",
        "this binary has no cloud command and opens no connection to any host; the Cloud page describes an optional service and contacts nothing",
      ),
    ),
    /* Every download there is, listed rather than implied by "no telemetry":
     * the embedding model on first use, and the two accelerator packs only
     * when their lane is on. Each says where it comes from, how large it is,
     * when it would happen, and whether it already has. */
    el(
      "div",
      { class: "card pad" },
      el(
        "div",
        { class: "head" },
        el("span", { class: "card-title", text: "Downloads" }),
        el("span", { class: "spacer" }),
        el("span", {
          class: "meta",
          text: "everything semlith ever fetches, and when",
        }),
      ),
      dataTable({
        caption:
          "Every file semlith can download: what it is, where it comes from, its size, when the download happens, and whether it is on this machine already.",
        rows: data.downloads || [],
        columns: [
          { key: "what", label: "What", value: (r) => r.what },
          { key: "source", label: "From", value: (r) => r.source },
          { key: "bytes", label: "Size", value: (r) => r.bytes, render: (r) => bytes(r.bytes) },
          { key: "when", label: "When", value: (r) => r.when },
          {
            key: "cached",
            label: "Here",
            value: (r) => (r.cached ? "here" : "not downloaded"),
            render: (r) => pill(r.cached ? "here" : "not downloaded", r.cached ? "good" : null),
          },
        ],
      }).node,
    ),
    el(
      "div",
      { class: "grid scroller" },
      /* How to check the promise, and what the stores already hold: both are
       * things a reader does rather than reads, so they share a column. */
      el(
        "div",
        { class: "rows" },
        el(
          "div",
          { class: "card pad" },
          el("span", { class: "card-title", text: "Verify it yourself" }),
          el(
            "div",
            { class: "steps-list" },
            step(
              "1",
              "Ask the operating system what this process has open. Only loopback should appear.",
              "lsof -nP -p $(pgrep -f 'semlith start') -i",
            ),
            step(
              "2",
              "Watch every interface but loopback while you search. Nothing should appear.",
              "sudo tcpdump -i any -n 'not host 127.0.0.1 and not host ::1'",
            ),
            step(
              "3",
              "Arm the refusal. Anything that would reach the network exits instead, naming what it refused.",
              "semlith start --airgap",
            ),
            step("4", "Or pull the cable: the portal loads and searches with no network at all.", "ifconfig en0 down"),
            // The fifth step the design ends on, and the one that settles the
            // ledger: it is a table in a file on this disk, readable by anything
            // that reads SQLite, and countable without asking semlith.
            step(
              "5",
              "See the ledger for what it is: one local table, written by this machine and nothing else.",
              `sqlite3 ${data.store_home || "~/.semlith"}/stores/<name>/store.db 'select count(*) from retrievals'`,
            ),
          ),
        ),
        el(
          "div",
          { class: "card pad" },
          el(
            "div",
            { class: "head" },
            // The heading said "Scan" and the button beside it said "Scan",
            // which rendered as the word twice.
            el("span", { class: "card-title", text: "What is already stored" }),
            el("span", { class: "spacer" }),
            scanButton,
          ),
          el("p", {
            class: "subtitle",
            text: "The rules below decide what semlith will take in from now on. This checks what the stores are already holding: every file that semlith would refuse today — indexed before a rule widened, or before the credential content scan existed.",
          }),
          scanBox,
          scanNote,
        ),
      ),
      el(
        "div",
        { class: "rows" },
        /* The rules this release added, each with what the daemon found when it
         * looked. A page that states a policy is a page; a page that states a
         * policy and the reading behind it is something a reader can disagree
         * with, which is the only version worth putting on a Privacy page. */
        el(
          "div",
          { class: "card pad" },
          el("span", { class: "card-title", text: "The one outbound connection that exists" }),
          says(
            "The embedding model is downloaded once, on first index, and cached. ",
            mono("semlith upgrade"),
            " and ",
            mono("semlith add"),
            " reach the network only in the second you ask them to. ",
            mono("--airgap"),
            " refuses all three and exits naming what it refused.",
          ),
          copyField("SEMLITH_MODEL_CACHE=/media/usb/models semlith index ."),
          el(
            "div",
            { class: "chips" },
            pill(data.model_cached ? "model cached" : "model not downloaded", data.model_cached ? "good" : "warn"),
            el("span", {
              class: "meta",
              text: "granite-embedding-small-english-r2 · int8 · Apache-2.0",
            }),
          ),
        ),
        /* Fifth, between the outbound card and the session token, which is
         * where the design has it. It stood outside this grid entirely, so
         * the one switch on the page sat above the cards that explain what
         * the page promises rather than among them. */
        sessionReplayToggle(),
        el(
          "div",
          { class: "card pad" },
          el("span", { class: "card-title", text: "Session token" }),
          el("div", { class: "copyfield" }, tokenBox, el("div", { class: "actions" }, rotate)),
          rotateNote,
          says(
            "Generated at start, handed to this page once by the printed URL, and sent back as a ",
            mono(data.token_header),
            " header on every ",
            mono("/api"),
            " route. It is in no cookie: every port on localhost is the same site, so a cookie would travel to a page served by anything else on this machine, and a header will not. Shown truncated — no response carries it in full except the one that rotates it.",
          ),
          el("hr", { class: "rule" }),
          el("span", { class: "card-title", text: "Content-Security-Policy" }),
          codeBlock(data.csp),
          el("p", {
            class: "subtitle",
            text: `Host headers answered: ${(data.host_allowed || []).join(", ")}. Everything else gets 400.`,
          }),
        ),
      ),
    ),
    /* After the grid, at the page's full width. Inside either column it is a
     * narrow strip holding eight rules of prose, several screens tall on its
     * own, and the page scrolled almost entirely because of it. Across the
     * page its rules sit two or three abreast. */
    el(
      "div",
      { class: "card pad rules-card" },
      el(
        "div",
        { class: "head" },
        el("span", { class: "card-title", text: "Rules" }),
        el("span", { class: "spacer" }),
        pill(
          // What the badge is actually about. "All holding" read as a
          // statement about everything semlith is storing, three lines
          // above a scan that had found a private key in a store — the
          // rules are forward-looking, and this now says so.
          (data.rules || []).every((r) => r.ok)
            ? "holding for new writes"
            : "check the rows",
          (data.rules || []).every((r) => r.ok) ? "good" : "warn",
        ),
      ),
      el("p", {
        class: "subtitle",
        text: "What semlith refuses, and what this daemon found when it checked. Every row is a rule the binary enforces and a test that fails if it stops.",
      }),
      rulesBox,
      fixNote,
    ),
  );
}

// ----------------------------------------------------------------- about

/* Every name `--lang` accepts, with a tick on the ones the graph is extracted
 * from. It lives here rather than on a page of its own because it is a fact
 * about the binary, and a page that held one table was a click between the
 * reader and a list they wanted to scan. */
function langCard(languages, withEdges) {
  const edges = new Set(withEdges);
  return el(
    "div",
    { class: "card pad lang-card" },
    el(
      "div",
      { class: "rows tight" },
      el("span", { class: "card-title", text: `${languages.length} languages` }),
      el("span", {
        class: "subtitle",
        text:
          edges.size === languages.length
            ? `Search filters and the code graph read the same table, so the two cannot disagree. Every one carries graph edges.`
            : `Search filters and the code graph read the same table, so the two cannot disagree. ${edges.size} of ${languages.length} carry graph edges.`,
      }),
    ),
    el(
      "div",
      { class: "lang-grid" },
      languages.map((lang) => {
        const has = edges.has(lang.name);
        const names = (lang.extensions || [])
          .map((e) => `.${e}`)
          .concat(lang.filenames || []);
        return el(
          "div",
          { class: "lang-row" },
          // Every row is ticked, because every row is true of the thing the
          // tick says: the language is indexed and `--lang` selects it. A
          // language that also carries graph edges says so beside its name.
          // Since 0.17.0 that is all of them, and the mark stays rather than
          // being dropped as redundant: if a grammar is ever refused — a
          // copyleft licence is the case that would do it — the gap has to be
          // visible here rather than inferred from its absence.
          el("span", {
            class: "tick on",
            title: "indexed, and --lang selects it",
            text: "✓",
          }),
          el("span", { class: "name", text: lang.name }),
          has ? el("span", { class: "graph-mark", text: "graph" }) : null,
          el("span", { class: "spacer" }),
          el("span", { class: "exts", text: names.join(" ") }),
        );
      }),
    ),
    says(
      `All ${languages.length} are indexed and selectable with `,
      mono("--lang"),
      edges.size === languages.length
        ? ", and every one marked "
        : `. The ${edges.size} marked `,
      mono("graph"),
      edges.size === languages.length
        ? " carries symbols and edges as well, from a tree-sitter grammar. "
        : " carry symbols and edges as well, from a tree-sitter grammar; the rest are searched as text. ",
      mono("semlith languages"),
      " prints the same table. Extension and filename decide the language — file contents are never read to guess it, because a store is searched far more often than it is built. Those grammars are most of what the binary weighs, and no language was dropped to hit a size.",
    ),
  );
}

async function aboutView() {
  let about;
  let languages;
  try {
    [about, languages] = await Promise.all([api("/api/about"), api("/api/languages")]);
  } catch (e) {
    return el("div", { class: "view" }, pageHead("About"), error(e.message));
  }

  const row = (key, value) =>
    el(
      "div",
      { class: "kv pair" },
      el("span", { class: "k", text: key }),
      el("span", { class: "v", text: value }),
    );

  return el(
    "div",
    { class: "view" },
    pageHead("About", "One Rust binary. The portal you are reading is compiled into it."),
    el(
      "div",
      // Sized to its content rather than to the height left over, so the page
      // scrolls as one. With `grow` the two columns were capped at the
      // viewport and the language card — 46 rows — pushed the rest of the left
      // column out of sight with nothing to scroll it back.
      //
      // The language card is out of the columns entirely and full width under
      // them, the way the v4 About page lays it out. Inside a column its 46
      // rows stretched the row, and the models table beside it was cut off
      // mid-row by its own scroller with no way to tell that from a bug.
      { class: "grid two about-grid" },
      el(
        "div",
        { class: "rows" },
        el(
          "div",
          { class: "card pad" },
          row("Version", `${about.version} · store format ${about.format_version}`),
          row("Binary", `${about.binary} · ${bytes(about.binary_bytes)} · ${about.target}`),
          row("Bound to", about.bind),
          row("Store home", about.store_home),
          row("Model cache", about.model_cache),
          // `MCP revisions` stood here. The v4 design's About page has seven
          // facts and this is not one of them, and what it showed -- which
          // protocol revisions a client may negotiate -- is a thing an agent
          // settles in its handshake and a person never acts on. The route
          // still answers `revisions` for anything else that reads it.
          row("Source", `${about.license} · free and complete`),
          row("Uptime", `${Math.floor(about.uptime / 60)}m · pid ${about.pid}`),
        ),
      ),
      // The models table stood between these two. Forty-eight rows of a
      // catalogue, of which this machine has fetched one -- the design has no
      // place for it, and `semlith models` prints the same list for anyone who
      // wants it. `/api/models` is left answering and is now the one route with
      // no portal view; that exception is argued in `tests/portal.rs` and
      // recorded in `docs/compatibility.md` rather than assumed.
      langCard(languages.languages || [], about.graph_languages || []),
    ),
    el("p", {
      class: "subtitle",
      text: "The model is fixed when a store is created — vectors from two models are not comparable. To switch, delete the store and index again.",
    }),
  );
}

// --------------------------------------------------------------- welcome

function welcomeView() {
  const note = el("div", { class: "note" });
  const field = el("input", { type: "text", placeholder: "~/Documents/work" });

  /* The version beside the mark, as the v4 lockup has it. The stylesheet has
   * carried `.lockup .ver` since the design was ported; nothing rendered into
   * it, so the first screen never said which build was running. */
  const version = el("span", { class: "ver" });
  (async () => {
    try {
      const about = await api("/api/about");
      version.textContent = `v${about.version}`;
    } catch (_) {
      // The lockup reads fine without it; a failed probe is not worth a row.
    }
  })();

  const step = (num, title, what) =>
    el(
      "div",
      { class: "step" },
      el("span", { class: "eyebrow", text: num }),
      el("span", { class: "card-title", text: title }),
      el("span", { class: "what", text: what }),
    );

  return el(
    "div",
    { class: "welcome" },
    el("div", { class: "lockup" }, logoImage(38), el("span", { class: "name", text: "Semlith" }), version),
    el(
      "div",
      { class: "hero" },
      el(
        "div",
        { class: "titles" },
        el("h1", { class: "hero-title", text: "No stores yet" }),
        el("p", {
          class: "subtitle",
          text: "The daemon is running and holds nothing. Point it at a folder and it indexes everything a person could read in there — code, Markdown, PDFs, Office files, notebooks, books and mail.",
        }),
      ),
      el(
        "div",
        { class: "field tall" },
        el("span", { class: "prefix", text: "path" }),
        labelled("welcome-path", "Folder to index", field),
      ),
      note,
      el(
        "div",
        { class: "actions" },
        el("button", {
          class: "button",
          type: "button",
          text: "Index this folder",
          onclick: () => {
            const path = field.value.trim();
            if (!path) {
              note.className = "note bad";
              note.textContent = "Give a path first, or open the picker instead.";
              field.focus();
              return;
            }
            // Carried across rather than dropped: this button used to validate
            // a path and then open the Index view with an empty field.
            state.pendingPath = path;
            go("index");
          },
        }),
        el("button", {
          class: "button secondary",
          type: "button",
          text: "Open the folder picker",
          onclick: () => go("index"),
        }),
        // The v4 welcome offers this beside indexing, and the portal has had
        // the feature on Stores all along — it was simply unreachable from the
        // one screen that exists to get a first store open.
        el("button", {
          class: "button secondary",
          type: "button",
          text: "Adopt an existing .semlith",
          onclick: () => go("stores"),
        }),
        el("button", {
          class: "button secondary",
          type: "button",
          text: "Skip for now",
          // Stores, not About: skipping the first run means going to the page
          // this screen is standing in front of.
          onclick: () => go("stores"),
        }),
      ),
      el("hr", { class: "rule" }),
      el("span", { class: "eyebrow", text: "Or from a terminal" }),
      copyField("semlith index ~/Documents/work"),
      says(
        "No ",
        mono("--store"),
        " flag. It lands in ",
        mono("~/.semlith/stores/work"),
        " and is registered against that root.",
      ),
      // The ledger is on by default, so the screen that introduces the product
      // is where it has to be said — the v4 welcome says it here too.
      says(
        "Queries are recorded to a local file in that store and never leave this machine. Start with ",
        mono("semlith start --no-ledger"),
        " to skip recording.",
      ),
    ),
    el(
      "div",
      { class: "steps" },
      step("01", "It reads the folder", "Code, Markdown, PDF, Office, notebooks, HTML — chunked and embedded locally."),
      step("02", "It stays current", "The watcher re-embeds and re-extracts edges on every save."),
      step("03", "Your agents connect", "One HTTP endpoint for Claude Code, Codex, Cursor, Zed and the rest."),
      step("04", "It keeps a local record", "Records what agents retrieve, locally; --no-ledger to skip."),
    ),
    // The port is half the sentence: "loopback only" means nothing without the
    // address the reader can go and check.
    el("div", { class: "foot", text: `${location.host} · loopback only · no external asset` }),
  );
}

// ---------------------------------------------------------------- router

/* Filled by the release's own tasks: Impact (T02), Reports (T08) and Cloud
 * (T10). They exist from the shell task so the sidebar never holds an entry
 * that navigates to nothing. */

/* Map: the subsystems, as a list.
 *
 * Communities over the settled `calls` and `imports` edges — no prose, no
 * model, and no second picture beside the canvas. It sits outside the rail
 * because the rail is about whichever node is selected and this is about the
 * store, so selecting a node must not wipe it. */
function mapPanel() {
  const body = el("div", { class: "map-body" }, skeletonRows(4));
  const shown = el("div", { class: "map-shown" });

  (async () => {
    let data;
    try {
      data = await api("/api/map");
    } catch (e) {
      fill(body, error(e.message));
      return;
    }
    const list = data.communities || [];
    if (!list.length) {
      fill(body, el("div", { class: "rail-hint", text: "No call or import edges to group yet." }));
      return;
    }
    fill(
      body,
      list.map((community) =>
        el(
          "button",
          {
            class: "map-row",
            type: "button",
            // The busiest member is the handle on a community, so the row
            // goes where a reader would go next: that name, in Search.
            onclick: () => {
              state.pendingQuery = community.label;
              go("search");
            },
          },
          el(
            "span",
            { class: "line one" },
            el("span", { class: "label", text: community.label }),
            el("span", { class: "size", text: `${community.size} symbol${community.size === 1 ? "" : "s"}` }),
          ),
          el("span", {
            class: "line hubs",
            text: `hubs: ${community.hubs
              .map((hub) => (hub.path ? `${hub.name} · ${shortPath(hub.path)}:${hub.line}` : hub.name))
              .join(" · ")}`,
          }),
          el("span", {
            class: "line cross",
            text: community.cross
              ? `${community.label} → ${community.cross[0]} · ${community.cross[1]} settled edge${community.cross[1] === 1 ? "" : "s"}`
              : "nothing leaves this group",
          }),
        ),
      ),
    );
    fill(
      shown,
      el("span", {
        text: `Shown ${data.shown} of ${data.total} · communities over settled calls and imports edges`,
      }),
    );
  })();

  return el(
    "section",
    { class: "map-panel" },
    el("div", { class: "card-head" }, el("h2", { text: "Map" }), el("span", { class: "mono-chip", text: "subsystems" })),
    body,
    shown,
  );
}

/* Impact: the graph read backwards.
 *
 * The Graph page answers "what is around this"; this one answers "who would
 * notice if it changed", which is a different question with a different
 * shape — a list by hop rather than a picture. Under it sit the path finder
 * and Trace, because all three read the same edges and a reader who has just
 * seen who reaches a symbol is one question away from asking how.
 *
 * Two deviations from the v4 design, both because the design is a mock over
 * fixed sample data and this is a page over a store: there is a symbol box
 * and a real hop control (the mock's subject arrives only from a Graph-page
 * link and its `depth 4 · reverse` pill is decoration), and every reached row
 * carries its file and line, which the mock drops. */

/** The uppercase mono label the v4 cards put above a value.
 *
 * `.eyebrow` already is that label — a second class for one would be two
 * spellings of one thing in a stylesheet shared with semlith-cloud. */
function capLabel(text) {
  return el("span", { class: "eyebrow", text });
}

function statCell(label, value) {
  return el("div", { class: "impact-stat" }, capLabel(label), el("span", { class: "n", text: String(value) }));
}

/* Reverse reachability, drawn the way the Graph page draws the graph.
 *
 * This was a static painter: concentric rings, one per hop, drawn once and
 * redrawn only on a resize or a theme change. It said the hop count clearly and
 * said nothing else — the nodes could not be moved, hovered or picked, and a
 * ring with more symbols than fit at its radius simply counted the rest.
 *
 * The design paints this canvas with the same force simulation the Graph page
 * uses, and that is what the page is for: the same picture, the same
 * interactions, the same drift, on a subgraph instead of on the whole store. So
 * this is `graphCanvas` with an impact answer converted into its nodes and
 * edges, rather than a second painter with its own behaviour to keep in step.
 *
 * Hop is not lost by dropping the rings — every row carries it in the table
 * beside this, and each edge here is one hop, so the distance from the subject
 * is the number of edges to it.
 */

/* The most nodes worth laying out. `graphView` caps at 180 for a whole store;
 * this card is a third of that one's height, so it takes a third of its
 * budget. Nearest hops are kept, because a caller two hops away is the one the
 * reader is looking for. */
const IMPACT_NODE_CAP = 34;

function impactCanvas() {
  /* The Graph page's hover card, with what this answer knows about the node:
   * how far it is from the subject, where it lives, and which edge reached it. */
  const graph = graphCanvas({
    onHover: (node, x, y) => {
      if (!node) return tip.hide("impact");
      tip.atPoint(
        x,
        y,
        node.hop
          ? tipCard(node.name, "blue", [
              ["hops", node.hop],
              ["file", node.path ? `${shortPath(node.path)}:${node.line}` : "—"],
              ["reaches", `${node.via} · ${node.edge}`],
              ["confidence", node.confidence || "—"],
            ])
          : tipCard(node.name, "accent", [["hops", "0 · the subject"]]),
        "impact",
      );
    },
  });
  const caption = el("span", { class: "canvas-caption", text: "reverse reachability" });
  const wrap = el("div", { class: "card impact-canvas-card" }, graph.node, caption);

  /* An impact answer is a list of rows, each naming what it reaches through.
   * The canvas wants nodes and edges, so `via` becomes the other end: a row is
   * the edge `name -> via`, and the subject is the node every chain ends at. */
  function shape(subject, rows) {
    const index = new Map([[subject, 0]]);
    const nodes = [{ name: subject, kind: "symbol" }];
    const edges = [];
    const add = (name) => {
      if (index.has(name)) return index.get(name);
      if (nodes.length >= IMPACT_NODE_CAP) return null;
      index.set(name, nodes.length);
      nodes.push({ name, kind: "symbol" });
      return nodes.length - 1;
    };
    // Nearest first, so the cap drops the far edge of the answer rather than
    // whichever rows the store happened to return last.
    for (const row of [...rows].sort((a, b) => a.hop - b.hop)) {
      const from = add(row.name);
      // The first row to reach a node is its nearest, and the one the card
      // describes. The subject is never a row's `name`, so it keeps no hop.
      if (from !== null && from !== 0 && !nodes[from].hop) {
        Object.assign(nodes[from], {
          hop: row.hop,
          path: row.path,
          line: row.line,
          via: row.via,
          edge: row.kind || "calls",
          confidence: row.confidence,
        });
      }
      const to = add(row.via);
      if (from === null || to === null) continue;
      edges.push({ from, to, kind: row.kind || "calls", confidence: row.confidence });
    }
    return { nodes, edges, total: rows.length + 1 };
  }

  return {
    node: wrap,
    show(name, reached) {
      if (!name) return this.clear();
      const data = shape(name, reached || []);
      graph.draw(data);
      // The subject selected, as though it had been clicked: it is the one node
      // every other node on this canvas is measured from, and selecting it
      // lights its own edges as well as drawing it in the accent.
      graph.pick(name);
      const hidden = data.total - data.nodes.length;
      caption.textContent = hidden > 0
        ? `reverse reachability · ${hidden} beyond the ${IMPACT_NODE_CAP} drawn`
        : "reverse reachability";
      graph.start();
    },
    clear() {
      graph.draw({ nodes: [], edges: [], total: 0 });
      caption.textContent = "reverse reachability";
    },
  };
}

async function impactView() {
  await refreshStores();

  const results = el("div", { class: "impact-results" });
  const subject = el("span", { class: "impact-subject", text: "—" });
  const depthPill = el("span", { class: "pill warn", text: "depth 3 · reverse" });
  const rings = impactCanvas();
  /* The store the question is about. Carried from the Graph page's Blast
   * radius, which knows which store the symbol was picked in; the same name in
   * another open store is another symbol with another reach. */
  let store = state.impactStore || "";
  const scope = () => store;
  const pathCard = pathFinderCard(scope);
  const traceLane = traceCard(scope);
  const reached = statCell("Reached", 0);
  const files = statCell("Files", 0);
  const inferredCell = statCell("Inferred", 0);
  const stats = el("div", { class: "impact-stats" }, reached, files, inferredCell);
  const setStats = (a, b, c) => {
    reached.querySelector(".n").textContent = String(a);
    files.querySelector(".n").textContent = String(b);
    inferredCell.querySelector(".n").textContent = String(c);
  };

  const nameInput = el("input", {
    type: "search",
    placeholder: "A symbol's name, matched exactly",
    "aria-label": "Symbol",
    onkeydown: (e) => {
      if (e.key === "Enter") run();
    },
  });
  if (state.impactSymbol) nameInput.value = state.impactSymbol;

  const depthInput = el("input", {
    type: "number",
    min: "1",
    max: "10",
    value: "3",
    "aria-label": "Hops",
    class: "hops",
  });

  let allEdges = false;
  // The same control the path finder below carries, drawn the same way: two
  // looks for one named toggle on one page reads as two different things.
  const verified = chipToggle("Prefer verified edges", true, (on) => {
    allEdges = !on;
    if (nameInput.value.trim()) run();
  });

  const reach = el("button", { class: "button small", type: "button", text: "Reach", onclick: () => run() });
  const storeChip = el("span", { class: "chips" });
  function paintStore() {
    fill(
      storeChip,
      store
        ? el("button", {
            class: "chip sm",
            type: "button",
            "aria-pressed": "true",
            title: "Answering about this store only. Press to ask every open store.",
            text: `in ${store} ×`,
            onclick: () => {
              store = "";
              state.impactStore = "";
              paintStore();
              if (nameInput.value.trim()) run();
            },
          })
        : null,
    );
  }
  paintStore();

  /* Where to start, for a page opened with no question: the Graph is where a
   * symbol is usually found, and its Blast radius button lands here with the
   * symbol and its store already filled in. */
  function emptyImpact() {
    return el(
      "div",
      { class: "rows" },
      el("p", {
        class: "subtitle",
        text: "Type a symbol's exact name above and press Reach, or find it on the Graph page, pick it, and press Blast radius.",
      }),
      el("div", {}, el("button", { class: "button secondary small", type: "button", text: "Open Graph", onclick: () => go("graph") })),
    );
  }

  /* A real table row, in a real table.
   *
   * These were `div`s on a grid, and a grid is only a table for as long as
   * every row agrees about its tracks: a long symbol pushed its own row's
   * columns out of line with the heading above it, and the whole block read as
   * loose text rather than as an answer with columns. `table` is the element
   * for this, its heading sticks, and the browser sizes the columns from the
   * content instead of from a guess written in `fr` units. */
  function reachedRow(row) {
    return el(
      "tr",
      { class: "impact-row" },
      el("td", { class: "sym" }, el("code", { title: row.name, text: row.name })),
      // The call site, where the store recorded one: the line to open to see
      // the call, which can sit hundreds of lines below the definition. An
      // edge an older binary wrote has none, and the cell says so rather
      // than passing the definition off as the call. The whole path is in the
      // title, because the cell truncates it.
      row.at
        ? el("td", {
            class: "where path",
            title: `${row.path}:${row.at} · call in ${row.name}, defined at line ${row.line}`,
            text: `${shortPath(row.path)}:${row.at}`,
          })
        : el("td", {
            class: "where path",
            title: `${row.path}:${row.line} · no call-site line in this store; a re-index adds it`,
            text: `${shortPath(row.path)}:${row.line} (definition)`,
          }),
      el(
        "td",
        { class: "via" },
        el("span", { class: "one-line", text: `${row.via} · ${row.kind}` }),
        confidenceBadge(row.confidence),
      ),
      el("td", { class: "hops-n num", text: String(row.hop) }),
    );
  }

  // One table, split into a `tbody` per hop, rather than one table per hop:
  // the hops are groups of one answer and share its columns, so the heading is
  // written once and the group label rides a row that spans it.
  function hopGroup(hop, rows) {
    return el(
      "tbody",
      { class: "impact-block" },
      el(
        "tr",
        { class: "impact-hop-row" },
        el(
          "th",
          { colspan: "4", scope: "rowgroup" },
          el(
            "div",
            { class: "impact-hop" },
            el("h2", { text: `${hop} hop${hop === 1 ? "" : "s"}` }),
            el("span", { class: "muted", text: `${rows.length} definition${rows.length === 1 ? "" : "s"}` }),
          ),
        ),
      ),
      rows.map(reachedRow),
    );
  }

  async function run() {
    const name = nameInput.value.trim();
    state.impactSymbol = name;
    if (!name) {
      subject.textContent = "—";
      setStats(0, 0, 0);
      rings.clear();
      fill(results, emptyImpact());
      return;
    }
    const depth = Math.min(10, Math.max(1, Number(depthInput.value) || 3));
    depthInput.value = String(depth);
    subject.textContent = name;
    depthPill.textContent = `depth ${depth} · reverse`;
    fill(results, el("p", { class: "subtitle", text: "Reading…" }));
    let data;
    try {
      const query = new URLSearchParams({ name, depth: String(depth) });
      if (allEdges) query.set("all_edges", "1");
      if (store) query.set("store", store);
      data = await api(`/api/impact?${query}`);
    } catch (e) {
      rings.clear();
      setStats(0, 0, 0);
      fill(results, error(e.message));
      return;
    }
    const impact = data.impact;
    if (!impact || !impact.reached.length) {
      // A canvas still showing the last symbol's rings under a headline about
      // this one is the worst thing this page could draw.
      rings.clear();
      setStats(0, impact ? impact.files.length : 0, 0);
      fill(
        results,
        el("p", { class: "headline", text: data.headline || "nothing reaches that name" }),
        impact && impact.definitions.length
          ? el("p", { class: "note", text: "The symbol is indexed; nothing in an open store calls it." })
          : el("p", {
              class: "note",
              text: "No definition of that name is in an open store. Check the spelling, or index the repository that holds it.",
            }),
      );
      return;
    }

    const inferred = impact.reached.filter((r) => r.confidence === "inferred" || r.confidence === "ambiguous").length;
    const byHop = new Map();
    for (const row of impact.reached) {
      if (!byHop.has(row.hop)) byHop.set(row.hop, []);
      byHop.get(row.hop).push(row);
    }

    // The three figures belong to the `Changing` card in the left column, where
    // the design puts them, not to the table below. `Inferred` is the design's
    // own word for this cell; it read `Unsettled` here, which is the word this
    // project uses for the pair of confidences and not the word on the page.
    setStats(impact.reached.length, impact.files.length, inferred);
    rings.show(name, impact.reached);

    /* The two cards below now have a question to answer.
     *
     * The furthest caller to the symbol being changed: the longest chain this
     * answer contains, which is the one worth reading. Seeded only while both
     * fields are empty, so a question somebody typed is never overwritten. */
    const furthest = impact.reached[impact.reached.length - 1];
    if (furthest) {
      pathCard.seed(furthest.name, name);
      traceLane.seed(furthest.name, name);
    }

    fill(
      results,
      el("p", { class: "headline", text: data.headline }),
      impact.unqualified
        ? el("p", {
            class: "note",
            text: `Nothing named ${name} matched that owner, so this is every definition of the bare name.`,
          })
        : null,
      allEdges
        ? el("p", {
            class: "note hypothesis",
            text: "Walked names with several definitions. Rows badged ambiguous are a hypothesis, not a finding.",
          })
        : null,
      el(
        "div",
        { class: "card table-card impact-table" },
        el(
          "div",
          { class: "table-wrap" },
          el(
            "table",
            {},
            el("caption", { class: "sr-only", text: `Everything that reaches ${name}, nearest hop first` }),
            el(
              "thead",
              {},
              el(
                "tr",
                {},
                el("th", { text: "Reached symbol" }),
                el("th", { text: "Where" }),
                el("th", { text: "Via" }),
                el("th", { class: "num", text: "Hops" }),
              ),
            ),
            [...byHop.entries()].map(([hop, rows]) => hopGroup(hop, rows)),
          ),
        ),
      ),
      el(
        "div",
        { class: "card table-card impact-table" },
        el("div", { class: "table-head" }, el("h2", { text: "Files" })),
        el(
          "div",
          { class: "table-wrap" },
          el(
            "table",
            {},
            el("caption", { class: "sr-only", text: "The files those definitions are in" }),
            el(
              "thead",
              {},
              el(
                "tr",
                {},
                el("th", { text: "File" }),
                el("th", { class: "num", text: "Definitions" }),
                el("th", { class: "num", text: "Nearest" }),
              ),
            ),
            el(
              "tbody",
              // Its own class: a file is not an edge and carries no support
              // class, so a check counting unbadged hops must be able to tell
              // the two blocks apart.
              { class: "impact-block impact-files" },
              impact.files.map((file) =>
                el(
                  "tr",
                  { class: "impact-row" },
                  el("td", { class: "sym path", text: shortPath(file.path) }),
                  el("td", { class: "num", text: String(file.symbols) }),
                  el("td", { class: "num", text: `${file.nearest} hop${file.nearest === 1 ? "" : "s"}` }),
                ),
              ),
            ),
          ),
        ),
      ),
      impact.hidden
        ? el("p", { class: "note", text: `${impact.hidden} more left out at the limit of ${data.limit}.` })
        : null,
    );
  }

  fill(results, emptyImpact());
  if (state.impactSymbol) run();

  return el(
    "div",
    { class: "view" },
    pageHead("Impact", "Reverse reachability. What breaks if this changes — before the edit, not after the test run."),
    /* Two columns, as the design lays the page out: the answer on the left,
     * the two questions that follow from it on the right, and the canvas at
     * the top of the right column rather than spanning.
     *
     * The `Changing` row and the three figures were two unboxed page-wide
     * bands across the top; they are one card at the head of the left column,
     * which is where the design has them and is why the right column used to
     * read as a large empty area beside them.
     *
     * The search band is this project's own addition -- the design is a
     * prototype with one fixed subject and no way to ask about another -- and
     * it sits at the head of that same card rather than after it. Against a
     * real store there is nothing on this page until a name is typed, so a
     * control placed below the answer it produces would be a control nobody
     * finds. Recorded as a deliberate departure. */
    el(
      "div",
      { class: "impact-columns" },
      el(
        "div",
        { class: "impact-col" },
        el(
          "div",
          { class: "card pad impact-subject-card" },
          el(
            "div",
            { class: "impact-band" },
            nameInput,
            el("label", { class: "hops-label" }, el("span", { text: "hops" }), depthInput),
            verified,
            reach,
          ),
          el(
            "div",
            { class: "impact-subject-row" },
            capLabel("Changing"),
            subject,
            storeChip,
            el("span", { class: "spacer" }),
            depthPill,
          ),
          stats,
          // What the figures and the controls above them mean, once.
          el(
            "ul",
            { class: "impact-guide" },
            el("li", {}, el("b", { text: "Reached" }), " — every definition that calls, references or imports this one, directly or through others."),
            el("li", {}, el("b", { text: "Inferred" }), " — linked by a bare name match only. Corroborate before relying on it."),
            el("li", {}, el("b", { text: "Hops" }), " — how far back to read: 1 is direct callers only. Prefer verified edges leaves out names with several definitions."),
          ),
        ),
        results,
      ),
      el("div", { class: "impact-col" }, rings.node, pathCard, traceLane),
    ),
  );
}

/* What each of these two cards says before it has been asked anything.
 *
 * Both said `Name both ends of the chain.` -- the same sentence at four sites,
 * two of which sit one above the other on screen, which reads as a page that
 * has failed rather than as two cards waiting for a question. Each says what
 * it will do with the two names instead, and the two differ because the cards
 * do: one finds a chain, the other writes the chain out with its evidence. */
const PATH_EMPTY = "Two names, and this walks the edges between them.";
const TRACE_EMPTY = "Two names, and this writes the chain out with the lines that support it.";

/* The path finder, which until now answered only from the terminal.
 *
 * `Prefer verified edges` is the default and `Strict` says it out loud — the
 * same pair the CLI takes, and `Strict` wins over the other for the same
 * reason it does there. */
function pathFinderCard(scope) {
  const from = el("input", { type: "text", placeholder: "from", "aria-label": "Path from" });
  const to = el("input", { type: "text", placeholder: "to", "aria-label": "Path to" });
  const body = el("div", { class: "path-body" });
  const query = el("span", { class: "mono-chip", text: "max 6 hops" });

  let preferVerified = true;
  let strict = false;

  const verifiedChip = chipToggle("Prefer verified edges", true, (on) => {
    preferVerified = on;
    if (!on) strict = false;
    strictChip.setAttribute("aria-pressed", String(strict));
    run();
  });
  const strictChip = chipToggle("Strict", false, (on) => {
    strict = on;
    if (on) {
      preferVerified = true;
      verifiedChip.setAttribute("aria-pressed", "true");
    }
    run();
  });

  async function run() {
    const a = from.value.trim();
    const b = to.value.trim();
    if (!a || !b) {
      fill(body, el("p", { class: "subtitle", text: PATH_EMPTY }));
      return;
    }
    // `--strict` wins over `--all-edges`, exactly as it does on the command
    // line: asking for both is asking for the refusal you named explicitly.
    const allEdges = !preferVerified && !strict;
    query.textContent = `${a} → ${b} · max 6 hops`;
    fill(body, el("p", { class: "subtitle", text: "Walking…" }));
    let data;
    try {
      const q = new URLSearchParams({ from: a, to: b, depth: "6" });
      if (allEdges) q.set("all_edges", "1");
      if (scope && scope()) q.set("store", scope());
      data = await api(`/api/path?${q}`);
    } catch (e) {
      fill(body, error(e.message));
      return;
    }
    if (!data.path) {
      fill(
        body,
        el(
          "div",
          { class: "refusal" },
          el("p", {
            class: "mono",
            text: allEdges
              ? "Not connected within 6 hops by any edge in the store"
              : "Not connected within 6 hops by resolved edges",
          }),
          allEdges
            ? null
            : el("button", {
                class: "link",
                type: "button",
                text: "Show inferred chain",
                onclick: () => {
                  preferVerified = false;
                  strict = false;
                  verifiedChip.setAttribute("aria-pressed", "false");
                  strictChip.setAttribute("aria-pressed", "false");
                  run();
                },
              }),
        ),
      );
      return;
    }
    fill(body, chainBlock(data.path, data.summary));
  }

  fill(body, el("p", { class: "subtitle", text: PATH_EMPTY }));

  const node = el(
    "section",
    { class: "card pad path-card" },
    el("div", { class: "card-head" }, el("h2", { text: "Path finder" }), query),
    el("div", { class: "path-band" }, from, to, verifiedChip, strictChip, el("button", { class: "button small", type: "button", text: "Walk", onclick: () => run() })),
    body,
  );
  /* Seeded from the answer above it, once, and never over a typed question.
   *
   * Arriving here from the Graph page's `Blast radius` fills the reach box and
   * runs it, and these two cards sat empty underneath saying "two names" — with
   * the two names on screen a few inches above them. The design shows both
   * cards answering, which is only possible if something puts a question in
   * them. */
  node.seed = (a, b) => {
    if (from.value.trim() || to.value.trim()) return;
    if (!a || !b || a === b) return;
    from.value = a;
    to.value = b;
    run();
  };
  return node;
}

/** A pill that is on or off, in the two states the v4 design draws. */
function chipToggle(label, on, onchange) {
  const node = el("button", {
    // A chip, at the smaller of the design's two chip sizes. The pressed look
    // comes from `aria-pressed` through `.chip`, so a toggle and a filter chip
    // cannot drift apart -- and the state a screen reader is told is the same
    // attribute the stylesheet paints from, rather than a class beside it.
    class: "chip sm",
    type: "button",
    text: label,
    "aria-pressed": String(on),
    onclick: () => {
      const next = node.getAttribute("aria-pressed") !== "true";
      node.setAttribute("aria-pressed", String(next));
      onchange(next);
    },
  });
  return node;
}

/** The hops of a chain, with the seam drawn where the chain changed subject. */
function chainBlock(steps, summary) {
  const rows = [];
  steps.forEach((step, i) => {
    rows.push(
      el(
        "div",
        { class: "hop" },
        el("span", { class: "n", text: String(i + 1) }),
        el("span", { class: "end", text: `${step.from} @ ${shortPath(step.from_path)}:${step.from_line}` }),
        el("span", { class: "arrow", "aria-hidden": "true", text: "→" }),
        el("span", { class: "end", text: `${step.to} @ ${shortPath(step.to_path)}:${step.to_line}` }),
        el("span", { class: "kind", text: step.kind }),
        confidenceBadge(step.confidence),
      ),
    );
    const next = steps[i + 1];
    if (next && (next.from_path !== step.to_path || next.from_line !== step.to_line)) {
      rows.push(
        el("div", { class: "seam" }, `seam · ${step.to}: ${Math.max(2, step.definitions)} definitions · continues from ${next.from} @ ${shortPath(next.from_path)}:${next.from_line}`),
      );
    }
  });
  const s = summary || {};
  let trailer = `${s.hops} hop${s.hops === 1 ? "" : "s"} · ${s.extracted} extracted · ${s.resolved} resolved · ${s.inferred} inferred · ${s.ambiguous} ambiguous`;
  if (s.seams) {
    const names = (s.ambiguous_names || []).map(([name, count]) => `${name}: ${count} definitions`).join(", ");
    trailer += ` · ${s.seams} through ambiguous names (${names})`;
  }
  return [
    ...rows,
    el("p", { class: "trailer mono", text: trailer }),
    s.hypothesis
      ? el("p", {
          class: "note hypothesis",
          text: "A hypothesis, not a finding. Read the seam before you rely on it.",
        })
      : null,
  ];
}

/* Trace: the same chain, as something a reader can paste into a review.
 *
 * The sentence, the hops, and one line of source per hop — each marked a
 * supporting fact or a candidate, from the hop's own support class. Nothing
 * here re-walks the graph: `/api/trace` reads the chain the finder produced. */
function traceCard(scope) {
  const from = el("input", { type: "text", placeholder: "from", "aria-label": "Trace from" });
  const to = el("input", { type: "text", placeholder: "to", "aria-label": "Trace to" });
  const body = el("div", { class: "trace-body" });
  let evidenceText = "";

  const copy = copyButton(() => evidenceText, "Copy as evidence", true);

  async function run() {
    const a = from.value.trim();
    const b = to.value.trim();
    if (!a || !b) {
      fill(body, el("p", { class: "subtitle", text: TRACE_EMPTY }));
      return;
    }
    fill(body, el("p", { class: "subtitle", text: "Reading…" }));
    let data;
    try {
      const q = new URLSearchParams({ from: a, to: b, depth: "6" });
      if (scope && scope()) q.set("store", scope());
      data = await api(`/api/trace?${q}`);
    } catch (e) {
      fill(body, error(e.message));
      return;
    }
    const trace = data.trace;
    evidenceText = data.evidence || "";
    const parts = [capLabel("Answer"), el("p", { class: "answer", text: trace.answer })];
    if (trace.chain && trace.chain.steps.length) {
      parts.push(capLabel("Chain"));
      parts.push(...chainBlock(trace.chain.steps, trace.chain.summary));
      parts.push(capLabel("Supporting lines"));
      for (const line of trace.lines) {
        parts.push(
          el(
            "div",
            { class: "support" },
            el("code", { text: line.at ? `${line.at}   ${line.code}` : "no call line recorded" }),
            el(
              "div",
              { class: "support-foot" },
              el("span", { class: "hop-of", text: line.hop }),
              el("span", {
                class: `mark ${line.mark.startsWith("candidate") ? "candidate" : "fact"}`,
                text: line.mark,
              }),
            ),
          ),
        );
      }
    }
    fill(body, ...parts);
  }

  fill(body, el("p", { class: "subtitle", text: TRACE_EMPTY }));

  const node = el(
    "section",
    { class: "card pad trace-card" },
    el(
      "div",
      { class: "card-head" },
      el("h2", { text: "Trace" }),
      el("span", { class: "mono-chip", text: "evidence view" }),
      el("span", { class: "spacer" }),
      copy,
    ),
    el("div", { class: "path-band" }, from, to, el("button", { class: "button small", type: "button", text: "Trace", onclick: () => run() })),
    body,
  );
  // Seeded like the path finder above it, and for the same reason. See there.
  node.seed = (a, b) => {
    if (from.value.trim() || to.value.trim()) return;
    if (!a || !b || a === b) return;
    from.value = a;
    to.value = b;
    run();
  };
  return node;
}

/* Reports: five, generated here, exported in five formats.
 *
 * The page asks `/api/report` for the text and hands it to the viewer — the
 * export is not a second renderer, so a file saved from here and one written
 * by `semlith report` are the same bytes. */
/* The five reports, as the v4 picker draws them: what each one is, who reads
 * it, and what it answers once it is generated. The third line is the design's
 * `who` — a report nobody can name a reader for is a report nobody asks for. */
const REPORTS = [
  [
    "savings",
    "Retrieval savings",
    "Tokens the agents did not read, counted from the ledger instead of claimed.",
    "for whoever approves the spend",
    "What retrieval actually saved, per client, with the arithmetic shown.",
  ],
  [
    "access",
    "AI access audit",
    "Which agent read which file, when, in a hash-chained record nothing can quietly edit.",
    "for security review and AI-use policy",
    "Exactly what the assistants were shown, session by session.",
  ],
  [
    "change",
    "Change brief",
    "Blast radius for what this machine re-read, written as a note you paste into the pull request.",
    "for the reviewer, before the merge",
    "What changed, what reaches it, and which callers the tests cover.",
  ],
  [
    "health",
    "Index health",
    "Stale files, roots never indexed, formats skipped, and how much of the tree the graph covers.",
    "for the person who owns the store",
    "Whether the answers your agents get are current, and where the index has holes.",
  ],
  [
    "gaps",
    "Knowledge gaps",
    "Questions the agents asked that your corpus could not answer well — a to-do list for documentation.",
    "for whoever writes the docs",
    "Asked and not answered, called and not found, and the names that mislead.",
  ],
];

const REPORT_FORMATS = [
  ["markdown", "Markdown", "md", "text/markdown"],
  ["csv", "CSV", "csv", "text/csv"],
  ["json", "JSON", "json", "application/json"],
  ["html", "HTML", "html", "text/html"],
  /* The fifth format is bytes, not text. `/api/report?format=pdf` answers
   * `application/pdf` outside the JSON envelope the other four ride in, so it
   * is fetched as a blob and named in the preview rather than painted into it:
   * a PDF rendered as characters is a screenful of noise with a filename. */
  ["pdf", "PDF", "pdf", "application/pdf"],
];

/* The design's four window chips, and the name `/api/report` takes for each.
 *
 * `report::WINDOWS` also has `all`, and naming no window at all is a fifth
 * behaviour again — but the design draws neither, and a chip this page invents
 * is a chip the design cannot be checked against. Leaving a store's whole
 * history unreachable from here costs nothing a reader can see, because the
 * three reports that count over it ignore the window anyway. */
const REPORT_WINDOWS = [
  ["day", "24 hours"],
  ["week", "7 days"],
  ["month", "30 days"],
  ["quarter", "This quarter"],
];

/* The two reports that carry a date to narrow by.
 *
 * `report::generate_over` windows `access` and `change` and nothing else; the
 * other three are lifetime aggregates over what is on disk now, and
 * `Window::note` makes each of them say so in its own heading. The chips
 * follow that rather than offering a dial that moves no figure — a control
 * that cannot change the file it claims to change is worse than no control. */
/* The reports whose figures a window actually moves.
 *
 * `savings` and `gaps` joined these in 0.27.0. Every row in the ledger carries
 * `at` and always has; what was missing was a reader that took a bound, so the
 * page drew four window chips over the savings figure, disabled them, and
 * explained why the control it had just drawn could not work. The readers take
 * a bound now.
 *
 * `health` is still out, and genuinely: it is the index as it stands — files,
 * chunks, stale rows — and none of those figures has a date to narrow by. */
const WINDOWED_KINDS = ["access", "change", "savings", "gaps"];

/* The design's cadence chips, as the interval `schedule::Schedule` holds.
 *
 * The record's field is `every_seconds` and these are shortcuts over it, not a
 * smaller vocabulary: a cadence no chip spells is still a cadence, and
 * `semlith schedule add --every` can still say it. The design's fourth chip,
 * `On git commit`, is not here — nothing in this binary hooks a commit, and a
 * chip for it would be the one kind of lie this page exists to refuse. */
const REPORT_CADENCES = [
  ["Daily", 86400],
  ["Weekly", 604800],
  ["Monthly", 2592000],
];

/** An interval in seconds, as the design's cadence blocks read it. */
/* A chip is one line and cannot hold an absolute path. The design's chip reads
 * `~/.semlith/reports/`; a real store home is longer than that, so the tail is
 * what the chip shows and the whole path travels in its title and in the spec
 * line under the button. */
function tailPath(path) {
  const parts = String(path).split("/").filter(Boolean);
  return parts.length > 2 ? `…/${parts.slice(-2).join("/")}/` : `${path}/`;
}

function everyWords(seconds) {
  const hit = REPORT_CADENCES.find(([, every]) => every === seconds);
  if (hit) return hit[0].toLowerCase();
  const plural = (count, unit) => `every ${count} ${unit}${count === 1 ? "" : "s"}`;
  if (seconds % 86400 === 0) return plural(seconds / 86400, "day");
  if (seconds % 3600 === 0) return plural(seconds / 3600, "hour");
  return plural(Math.max(1, Math.round(seconds / 60)), "minute");
}

/** How long until a unix second, in words. `when()` only looks backwards. */
function until(unix) {
  if (!unix) return "not scheduled";
  const seconds = unix - Math.floor(Date.now() / 1000);
  if (seconds <= 0) return "due now";
  if (seconds < 60) return `in ${seconds}s`;
  if (seconds < 3600) return `in ${Math.round(seconds / 60)}m`;
  if (seconds < 86400) return `in ${Math.round(seconds / 3600)}h`;
  return `in ${Math.round(seconds / 86400)}d`;
}

/* Reports, in the v4 shape: pick a type, set it up, read the result.
 *
 * The page used to be five cards each carrying its own Generate button and its
 * own row of four format buttons — twenty-five controls for five reports, and
 * a preview at the bottom that could be showing any of them. The design's
 * shape is one selection and one builder: the picker says which report, the
 * builder says how, and the preview is the one it is about.
 *
 * Window and Scope are here now because `/api/report` takes them, and so are
 * two of the design's three toggles: `Hash the query text` replaces every query
 * with a digest of it wherever one reaches the page, and `Attach retrieved
 * excerpts` unrolls the access report's session lines into the retrievals
 * behind them. The design draws a third, `Sign the report`, and it is not here:
 * signing needs a key, and where that key lives, how it rotates and what a
 * reader checks it against are decisions this product has not made. A switch
 * with nothing behind it is a promise the file does not keep, so the option is
 * gone rather than drawn and disabled. */
async function reportsView() {
  await refreshStores();

  /* The real store home, so the paths this page prints are the daemon's own
   * rather than `~/.semlith/` assumed. A home the page cannot read leaves the
   * paths out rather than guessing at them. */
  let home = "";
  try {
    home = (await api("/api/about")).store_home || "";
  } catch (_) {
    /* The page still works; only the paths in two footers go unnamed. */
  }
  const reportDir = home ? `${home}/reports/` : "the store home's reports/ directory";
  const schedulesFile = home ? `${home}/schedules.json` : "schedules.json in the store home";

  let kind = REPORTS[0][0];
  let format = REPORT_FORMATS[0][0];
  let model = MODEL_PRICES[0];
  let span = "month";
  /* Empty is every open store, which is exactly what `/api/report` means by no
   * `scope` — so "all stores" is the absence of a narrowing, not a value. */
  let scope = [];
  // The two content toggles, off to begin with, which is the request 0.26.x
  // made and the file it produced.
  let redact = false;
  let excerpts = false;
  let body = "";
  let file = null;
  let report = null;
  /* What this page actually wrote, this session. Not the daemon's directory —
   * see the Written reports card for why that list cannot be read from here. */
  let written = [];

  const def = () => REPORTS.find(([id]) => id === kind);
  const fmt = () => REPORT_FORMATS.find(([id]) => id === format);
  const ext = () => fmt()[2];
  const windowed = () => WINDOWED_KINDS.includes(kind);
  const spanName = () => (REPORT_WINDOWS.find(([id]) => id === span) || REPORT_WINDOWS[2])[1];
  const scopeWords = () => (scope.length ? scope.join(", ") : "all stores");
  /* The date in the filename is the one inside the file, taken off the report
   * the daemon generated, so the two cannot disagree. */
  const stamp = () => (report && report.generated ? report.generated.slice(0, 10) : "");
  const baseName = () => `${kind}${stamp() ? `-${stamp()}` : ""}.${ext()}`;

  const picker = el("div", { class: "report-types" });
  const builder = el("div", { class: "card pad report-builder" });
  const builderProblem = el("div", { class: "note bad" });
  // Plain, not bold: in the design this is a label on the previewed file, at
  // the body's own weight and 12px mono. See `.name.plain`.
  const previewName = el("span", { class: "name plain" });
  const previewBody = el("pre", { class: "report-text" });
  const previewMeta = el("span", { class: "meta" });
  const savingsCard = el("section", { class: "card pad savings-card" });
  const gapsCard = el("section", { class: "card pad gaps-card" });
  const writtenCard = el("section", { class: "card written-card" });
  const scheduleCard = el("section", { class: "card pad schedules-card" });
  const cliCard = el("section", { class: "card pad report-cli-card" });

  /* A labelled row of chips, the shape all five chip groups on this page take.
   * `inline` is the design's 10.5px eyebrow, for the two groups that sit beside
   * a heading rather than above a column of controls. */
  function chipGroup(label, chips, options) {
    const { inline, note, group } = options || {};
    return el(
      "div",
      {
        class: inline ? "chip-group inline" : "chip-group",
        // A name a test can hold on to. The eyebrow is the label a reader
        // sees and is free to be reworded; this is not.
        "data-group": group || label.toLowerCase(),
      },
      el("span", { class: inline ? "eyebrow sm" : "eyebrow", text: label }),
      el("div", { class: "filters" }, chips),
      note ? el("p", { class: "subtitle", text: note }) : null,
    );
  }

  function chip(label, on, onclick, why) {
    return el("button", {
      class: "chip",
      type: "button",
      text: label,
      "aria-pressed": String(on),
      disabled: !onclick,
      title: why || null,
      onclick: onclick || null,
    });
  }

  // ------------------------------------------------------------- the report

  /** Generate the chosen report and show it. One request, whose bytes are then
   * what Copy copies, what Export writes and what Save to disk saves — so the
   * four cannot disagree, and none of them is a second renderer. */
  async function generate() {
    previewName.textContent = `reports/${baseName()}`;
    previewMeta.textContent = "Generating…";
    fill(previewBody, "");
    body = "";
    file = null;
    report = null;
    const query = new URLSearchParams({ kind, format, model: model[0] });
    // Omitted rather than sent and ignored: a report that cannot honour a
    // window should not have one in the URL that produced it.
    if (windowed()) query.set("window", span);
    for (const name of scope) query.append("scope", name);
    // Sent only when on, so the URL that produced a plain report is the URL
    // 0.26.x would have produced.
    if (excerpts) query.set("excerpts", "1");
    if (redact) query.set("redact", "1");
    try {
      if (format === "pdf") {
        // The one format outside the JSON envelope: bytes, fetched as bytes.
        const response = await fetch(`/api/report?${query}`, {
          credentials: "omit",
          headers: authed(),
        });
        if (!response.ok) throw new Error(response.statusText || "request failed");
        file = await response.blob();
      } else {
        const data = await api(`/api/report?${query}`);
        body = data.text;
        report = data.report;
        file = new Blob([body], { type: `${fmt()[3]};charset=utf-8` });
      }
    } catch (e) {
      // The failure goes beside the controls that caused it, not into the
      // preview: the preview is the file, and an error is not one.
      previewMeta.textContent = "";
      previewBody.textContent = "";
      fill(builderProblem, error(e.message));
      paintAll();
      return;
    }
    fill(builderProblem);
    previewName.textContent = `reports/${baseName()}`;
    if (format === "pdf") {
      previewBody.textContent =
        `${bytes(file.size)} of PDF. A page cannot show it as text; ` +
        `Export and Save to disk write these exact bytes.`;
    } else {
      previewBody.textContent = body;
    }
    paintMeta();
    paintAll();
  }

  /* The three things the footer states, each read off the file that exists
   * rather than estimated from the preview:
   *   rows shown  — every `Table` block's row count in the generated report,
   *                 which is the rows in the document, not the rows on screen;
   *   full file   — `Blob.size`, the byte length Export hands the browser;
   *   format      — the format the request asked for and the envelope echoed.
   * The design's fourth is a signature, and it is not here: nothing in this
   * binary signs a report, and a footer reading `unsigned` on every file for
   * ever is a column about a feature rather than about the document.
   * The trailing clause is the design's and is true of all three: every byte
   * above was produced on this machine. */
  function paintMeta() {
    const rows = ((report && report.blocks) || [])
      .filter((block) => block.block === "table")
      .reduce((total, block) => total + block.rows.length, 0);
    const size = file ? bytes(file.size) : "—";
    const parts = [
      format === "pdf" ? "rows not counted in a PDF" : `${rows} rows shown`,
      `full file ${size}`,
      format,
      "written to disk, never uploaded",
    ];
    previewMeta.textContent = parts.join(" · ");
  }

  const copy = copyButton(() => body, "Copy", true);

  const exportButton = el("button", {
    // The design's 26px ink shape: the smallest solid control on the page, and
    // the toolbar's own action rather than the page's primary one.
    class: "button tiny ink",
    type: "button",
    text: "Export",
    onclick: () => {
      if (!file) return;
      offerDownload(baseName(), file, fmt()[3]);
    },
  });

  /* The design's accent action. It saves the file and logs it; `Export` beside
   * the filename above saves it and says nothing, which is the split the design
   * draws — its own Save to disk writes no file at all and only prepends a row.
   *
   * It is the browser's save, not the daemon's: no route on this machine writes
   * a report into `reports/`, so a button claiming to fill that directory would
   * be claiming something no request from this page can do. The picker API was
   * tried here and taken out again — `showSaveFilePicker` exists in a headless
   * browser and its promise then never settles, which is a button that silently
   * does nothing for ever. */
  const saveButton = el("button", {
    class: "button",
    type: "button",
    text: "Save to disk",
    title: "Saves through the browser and lists it below",
    onclick: () => {
      if (!file) return;
      offerDownload(baseName(), file, fmt()[3]);
      record(baseName(), file.size);
    },
  });

  /** Remember a file this page really wrote, for the Written reports table. */
  function record(name, size) {
    written.unshift({
      name,
      title: def()[1],
      span: report ? report.window : "",
      size,
      at: Math.floor(Date.now() / 1000),
    });
    // The design caps its log at six rows once anything is saved.
    written = written.slice(0, 6);
    paintWritten();
  }

  // ------------------------------------------------------------- the builder

  function paintPicker() {
    fill(
      picker,
      REPORTS.map(([id, title, blurb, who]) =>
        el(
          "button",
          {
            class: `report-type${id === kind ? " on" : ""}`,
            type: "button",
            "aria-pressed": String(id === kind),
            onclick: () => {
              kind = id;
              paintPicker();
              generate();
            },
          },
          el("span", { class: "name", text: title }),
          el("span", { class: "what", text: blurb }),
          el("span", { class: "for-whom", text: who }),
        ),
      ),
    );
  }

  function paintBuilder() {
    const [, title, , , answers] = def();
    /* Under the title, the report's own sentence about the span it was taken
     * over — `Window::note`, which for savings, health and gaps says outright
     * that it counts over a store's whole history and has no date to narrow
     * by. That is the page's one statement about the window, and it comes from
     * the file rather than from the chip that is pressed. */
    const spanLine = report ? report.window : answers;
    fill(
      builder,
      el(
        "div",
        { class: "titles" },
        el("span", { class: "card-title", text: title }),
        el("p", { class: "subtitle", text: answers }),
        el("p", { class: "subtitle span-note", text: spanLine }),
      ),
      chipGroup(
        "Window",
        REPORT_WINDOWS.map(([id, label]) =>
          chip(
            label,
            windowed() && id === span,
            windowed()
              ? () => {
                  span = id;
                  generate();
                }
              : null,
            windowed() ? null : `${title} has no date to narrow by`,
          ),
        ),
      ),
      chipGroup("Scope", [
        chip("all stores", scope.length === 0, () => {
          scope = [];
          generate();
        }),
        ...state.stores.map((store) =>
          chip(store.name, scope.includes(store.name), () => {
            scope = scope.includes(store.name)
              ? scope.filter((name) => name !== store.name)
              : scope.concat(store.name);
            generate();
          }),
        ),
      ]),
      chipGroup(
        "Format",
        REPORT_FORMATS.map(([id, label]) =>
          chip(label, id === format, () => {
            format = id;
            generate();
          }),
        ),
      ),
      el(
        "div",
        { class: "report-toggles" },
        [
          [
            "Hash the query text",
            "Keeps the who, when and which-file; drops what was asked.",
            () => redact,
            (on) => {
              redact = on;
            },
          ],
          [
            "Attach retrieved excerpts",
            "The exact lines each agent was shown. Larger file.",
            () => excerpts,
            (on) => {
              excerpts = on;
            },
          ],
        ].map(([label, note, get, set]) => {
          const node = el(
            "button",
            { class: "report-toggle", type: "button", "aria-pressed": String(get()) },
            el("span", { class: "knob" }),
            el(
              "span",
              { class: "stack" },
              el("span", { class: "label", text: label }),
              el("span", { class: "why", text: note }),
            ),
          );
          node.onclick = () => {
            const next = node.getAttribute("aria-pressed") !== "true";
            node.setAttribute("aria-pressed", String(next));
            set(next);
            generate();
          };
          return node;
        }),
      ),
      builderProblem,
    );
  }

  // ------------------------------------------------- the savings breakdown

  /** One fact out of the generated report, by the label `report.rs` gives it. */
  function fact(label) {
    for (const block of (report && report.blocks) || []) {
      if (block.block !== "facts") continue;
      const hit = block.facts.find(([name]) => name === label);
      if (hit) return hit;
    }
    return null;
  }

  function factValue(label, fallback) {
    const hit = fact(label);
    return hit ? hit[1] : fallback || "—";
  }

  function tableStarting(prefix) {
    return ((report && report.blocks) || []).find(
      (block) => block.block === "table" && block.title.startsWith(prefix),
    );
  }

  function trustBadge(text, tone) {
    return el("span", { class: `trust ${tone}`, text });
  }

  function paintSavings() {
    const table = tableStarting("What was not read");
    if (!table) {
      fill(savingsCard, el("span", { class: "card-title", text: "Retrieval savings" }), empty("Nothing generated yet."));
      return;
    }
    const chain = fact("Ledger chain");
    fill(
      savingsCard,
      el(
        "div",
        { class: "savings-head" },
        el("span", { class: "card-title", text: "Retrieval savings" }),
        el("span", { class: "spacer" }),
        /* The design puts the model chips here, in this card's header, rather
         * than in the builder — they price one report, not all five. */
        chipGroup(
          "Model",
          MODEL_PRICES.map((price) =>
            chip(price[0], price[0] === model[0], () => {
              model = price;
              generate();
            }),
          ),
          { inline: true },
        ),
        /* The design prints `prices as of 2026-09-01`. This binary's prices
         * are `report::PRICES`, written into the source with no date on them,
         * so what is stated here is the rate itself — a number the reader can
         * check against the arithmetic above it. */
        el("span", { class: "mono-chip", text: `$${model[1].toFixed(2)} per Mtok, input` }),
        el("span", { class: "rule" }),
        chipGroup(
          "Cycle",
          REPORT_WINDOWS.slice(1).map(([, label]) => chip(label, false, null, "This report has no date to narrow by")),
          { inline: true },
        ),
      ),
      el(
        "div",
        { class: "savings-lines" },
        table.rows.map(([name, tokens, cost, rule]) =>
          el(
            "div",
            { class: "savings-line" },
            el(
              "div",
              { class: "row" },
              el("span", { class: "name", text: name }),
              el("span", { class: "spacer" }),
              el("span", { class: "tok", text: tokens }),
              el("span", { class: "amount", text: cost || "—" }),
            ),
            el("p", { class: "subtitle", text: rule }),
          ),
        ),
      ),
      el(
        "div",
        { class: "savings-net" },
        el("span", { class: "name", text: "Net" }),
        el("span", { class: "spacer" }),
        el("span", { class: "tok", text: factValue("Net") }),
        el("span", { class: "amount", text: factValue("Net cost avoided") }),
      ),
      el(
        "div",
        { class: "trust-strip" },
        trustBadge(`coverage ${factValue("Coverage")}`, "blue"),
        trustBadge(`zero-hit ${factValue("Zero-hit")}`, "plain"),
        trustBadge(`refunds ${factValue("Refund rate")}`, "plain"),
        trustBadge(`p50 ${factValue("p50")} · p95 ${factValue("p95")}`, "plain"),
        trustBadge(`tier ${factValue("Tier")}`, "blue"),
        trustBadge(
          `ledger ${chain ? chain[2] : "no rows yet"} · ${factValue("Ledger chain", "unknown")}`,
          chain && chain[1] === "verifies" ? "green" : "amber",
        ),
      ),
      /* The design's footnote says the baseline is priced at the cache-write
       * rate and that a reconciliation states a drift against Claude's own
       * counter. Neither is true of this binary: `report::PRICES` is input
       * pricing, said so in its own doc comment, and nothing reconciles
       * anything. What is written here is what the numbers above actually are. */
      el("p", {
        class: "subtitle",
        text:
          "Priced at input rates, because a retrieval is something an agent reads. " +
          "Token counts are this store's own tokenizer; a model's counter will differ, " +
          "and nothing here reconciles the two.",
      }),
    );
  }

  // ------------------------------------------------------------ the gaps card

  function paintGaps() {
    const several = fact("Names that mislead");
    fill(
      gapsCard,
      el(
        "div",
        { class: "card-head" },
        el("h2", { text: "Names that mislead" }),
        el("span", { class: "mono-chip", text: several ? `${several[1]} names` : "—" }),
      ),
      several ? el("p", { class: "subtitle", text: several[2] }) : null,
      /* The design lists each name with its definition count and how many
       * hypothesis chains it sits on. `store::names_with_several_definitions`
       * returns a count and nothing else, and no route breaks it down, so the
       * rows are absent rather than invented. */
      empty(
        "Which names, and how many chains each sits on, is not in the gaps report — " +
          "the store counts them without listing them.",
      ),
    );
  }

  // -------------------------------------------------------- written reports

  function paintWritten() {
    const head = (text, right) =>
      el("th", { class: right ? "right" : null, scope: "col", text });
    fill(
      writtenCard,
      el(
        "div",
        { class: "written-head" },
        el("span", { class: "card-title", text: "Written reports" }),
        el("span", { class: "mono-chip", text: reportDir }),
      ),
      el(
        "div",
        { class: "scroll-x" },
        el(
          "table",
          { class: "written-table" },
          el(
            "thead",
            null,
            el("tr", null, head("Written"), head("Report"), head("Window"), head("Size", true), head("When", true)),
          ),
          el(
            "tbody",
            null,
            written.map((row) =>
              el(
                "tr",
                null,
                el("td", { class: "file", text: row.name }),
                el("td", { class: "type", text: row.title }),
                el("td", { class: "window", text: row.span }),
                el("td", { class: "size right", text: bytes(row.size) }),
                el("td", { class: "when right", text: when(row.at) }),
              ),
            ),
          ),
        ),
      ),
      /* Not the daemon's directory. Nothing serves a listing of it — there is
       * no route that reads `reports/`, and `/api/dirs` answers names without
       * a size or a time, which is three of these five columns empty. What is
       * listed is what this page wrote, which it knows exactly. */
      written.length
        ? null
        : empty(
            `Nothing written from this page yet. What the daemon has already put in ${reportDir} ` +
              "is not listed here: no route reads that directory.",
          ),
    );
  }

  // ---------------------------------------------------------- the schedules

  let schedules = [];
  let scheduleProblem = "";
  /* A refused write, kept apart from a missing route: the list is reloaded
   * after every write, so a refusal folded into the same slot would be wiped
   * by the very read that followed it and the button would look inert. */
  let scheduleRefused = "";
  let cadence = REPORT_CADENCES[2][1];
  const destinations = [[tailPath(`${home}/reports`), home ? `${home}/reports` : ""]].concat(
    state.stores.map((store) => [`${store.name} root`, store.dir]),
  );
  let destination = destinations[0][1];

  async function loadSchedules() {
    try {
      const data = await api("/api/schedules");
      schedules = Object.entries(data.schedules || {}).map(([id, one]) => ({ id, ...one }));
      scheduleProblem = "";
    } catch (e) {
      schedules = [];
      scheduleProblem = e.message;
    }
    paintSchedules();
  }

  /* Every change is a POST to the one route, naming the verb `semlith
   * schedule` names. The page holds no schedule state of its own: the daemon
   * owns the file, and a card that remembered what it asked for would be a
   * second copy of what a schedule is. */
  async function writeSchedule(sent) {
    scheduleRefused = "";
    try {
      await post("/api/schedules", sent);
    } catch (e) {
      // The daemon names what would have worked -- a destination that is not a
      // directory, a kind this binary does not have. Printed as it was said.
      scheduleRefused = e.message;
    }
    // Reloaded rather than patched in place: the card states what the daemon
    // holds, and the daemon is the only thing that knows what it now holds.
    loadSchedules();
  }

  function scheduleRow(one) {
    const title = (REPORTS.find(([id]) => id === one.kind) || [, one.kind])[1];
    const where = String(one.dir || "");
    const spec = [
      everyWords(one.every_seconds),
      one.format,
      one.stores && one.stores.length ? one.stores.join(", ") : "all stores",
    ].join("  ·  ");
    return el(
      "div",
      { class: "schedule-row" },
      el("button", {
        class: `schedule-knob${one.enabled ? " on" : ""}`,
        type: "button",
        disabled: Boolean(scheduleProblem),
        title: one.enabled ? "Pause this schedule" : "Resume this schedule",
        "aria-pressed": String(Boolean(one.enabled)),
        onclick: () => writeSchedule({ action: "set", id: one.id, enabled: !one.enabled }),
      }),
      el(
        "div",
        { class: "stack" },
        el("span", { class: "name", text: `${title} → ${where.split("/").filter(Boolean).pop() || where}` }),
        el("span", { class: "spec", text: `${spec}  →  ${where}` }),
        el("span", {
          class: "next",
          text: one.enabled ? `next run ${until(one.next_run)}` : "paused",
        }),
        /* The failure this feature will actually meet: a destination deleted,
         * renamed or unmounted. `Schedule::last_error` carries the whole chain
         * and the card prints it, because a schedule that says it is running
         * beside a folder that never fills is the worst outcome available. */
        one.last_error ? el("span", { class: "failed", text: `failed: ${one.last_error}` }) : null,
        one.last_path ? el("span", { class: "wrote", text: `last wrote ${one.last_path}` }) : null,
      ),
      el("span", { class: "spacer" }),
      el("span", { class: `state-pill ${one.enabled ? "on" : "paused"}`, text: one.enabled ? "on" : "paused" }),
      el("button", {
        class: "schedule-remove",
        type: "button",
        text: "×",
        disabled: Boolean(scheduleProblem),
        title: "Remove schedule",
        onclick: () => writeSchedule({ action: "remove", id: one.id }),
      }),
    );
  }

  function paintSchedules() {
    const [, title] = def();
    /* The design's sentence, with this build's values in it. The window clause
     * is dropped for the three reports that have no window, rather than
     * printed and then quietly ignored by the schedule it describes. */
    const describes =
      `Takes the report set up above — ${title.toLowerCase()}, ` +
      (windowed() ? `last ${spanName().toLowerCase()}, ` : "") +
      `${scopeWords()}, as ${fmt()[1]}. Change those and add again for a second schedule.`;
    fill(
      scheduleCard,
      el(
        "div",
        { class: "card-head" },
        el("h2", { text: "Schedules" }),
        /* The design says `crontab in ~/.semlith/schedules.toml`. The file is
         * `schedules.json`, it is not a crontab, and its directory is whatever
         * home this daemon was started on. */
        el("span", { class: "mono-chip", text: `run by the daemon · ${schedulesFile}` }),
      ),
      scheduleProblem
        ? el("p", {
            class: "subtitle",
            text:
              `This daemon serves no schedules route (${scheduleProblem}), so the card is ` +
              "read-only until it does. The records themselves are real: `semlith schedule " +
              "list` reads the same file the daemon runs from.",
          })
        : null,
      scheduleRefused ? el("div", { class: "note bad" }, scheduleRefused) : null,
      schedules.length
        ? el("div", { class: "schedule-list" }, schedules.map(scheduleRow))
        : el("div", { class: "empty centred" }, "No schedules. Set up a report above, then add it here — the daemon writes the file while it is running."),
      el(
        "div",
        { class: "new-schedule" },
        el("span", { class: "eyebrow", text: "New schedule" }),
        el("p", { class: "subtitle strong", text: describes }),
        el(
          "div",
          { class: "filters" },
          REPORT_CADENCES.map(([label, every]) =>
            chip(label, every === cadence, () => {
              cadence = every;
              paintSchedules();
            }),
          ),
        ),
        el(
          "div",
          { class: "filters" },
          el("span", { class: "write-to", text: "write to" }),
          destinations.map(([label, path]) =>
            chip(
              label,
              path === destination,
              () => {
                destination = path;
                paintSchedules();
              },
              path,
            ),
          ),
        ),
        el(
          "div",
          { class: "filters" },
          el("button", {
            class: "button add-schedule",
            type: "button",
            /* The design's label carries a cron phrase — `1st of the month,
             * 08:00`. The record holds an interval and nothing else, so what
             * is promised here is the interval. */
            text: `Add schedule · ${everyWords(cadence)}`,
            disabled: Boolean(scheduleProblem) || !destination,
            onclick: () =>
              writeSchedule({
                action: "add",
                kind,
                format,
                model: model[0],
                every_seconds: cadence,
                dir: destination,
                // Null rather than absent for the three reports with no date
                // to narrow by: `Window::Unset` is what the record holds.
                window: windowed() ? span : null,
                stores: scope,
              }),
          }),
          el("span", {
            class: "mono-chip",
            text: `${everyWords(cadence)}  →  ${destination || "?"}/${kind}-<date>.${ext()}`,
          }),
        ),
      ),
      el("p", {
        class: "subtitle",
        text:
          "The daemon runs these while it is up and writes the file in place, so a report can be " +
          `a tracked file in the repo with a readable diff each month. The list is ${schedulesFile}; ` +
          "× removes one.",
      }),
    );
  }

  // ------------------------------------------------------------- the CLI card

  function paintCli() {
    const flags = [`--format ${format}`];
    if (windowed()) flags.push(`--window ${span}`);
    for (const name of scope) flags.push(`--scope ${name}`);
    if (kind === "savings") flags.push(`--model "${model[0]}"`);
    const out = home ? `${home}/reports/` : ".";
    // What the Copy button puts on the clipboard: one line, runnable as it is.
    const one = `semlith report ${kind} ${flags.join(" ")} --out ${out}`;
    /* What the card shows: the same command wrapped the way the design wraps
     * it, with a continuation at each break. A single line of eight flags ran
     * off the right edge of a half-width card and the reader saw `semlith
     * report savings --format mark…` — the flags are the part worth reading.
     * Backslashes rather than a soft wrap, so what is on screen is still
     * something that runs if it is selected by hand. */
    const wrapped = [`semlith report ${kind} \\`, ...flags.map((f) => `  ${f} \\`), `  --out ${out}`];
    const block = [
      ...wrapped,
      "",
      // The design's comment, and true of this build: the list is a file.
      "# the schedule list is a file — add, edit or delete by hand:",
      `semlith schedule add ${kind} \\`,
      `  --every ${cadence} --to ${destination || reportDir} \\`,
      `  --format ${format}`,
      "semlith schedule list   ·   semlith schedule remove 2",
    ].join("\n");
    fill(
      cliCard,
      el(
        "div",
        { class: "card-head" },
        el("h2", { text: "Same thing without the browser" }),
        el("span", { class: "spacer" }),
        /* The 26px copy sibling of Export: the same height, in the secondary
         * shape rather than filled with ink. The height comes from the card's
         * own rule, because `copyButton` is shared with eight other sites. */
        copyButton(one, "Copy", true),
      ),
      el("pre", { class: "code report-cli", text: block }),
      /* The design's footnote, verbatim, with `semlith_report` set in mono. It
       * is the only place the slug appears on this page: the v4 header has no
       * right-hand mono slug on Reports, unlike Impact. */
      el(
        "p",
        { class: "subtitle" },
        "The agent can ask for one too: ",
        mono("semlith_report"),
        " returns the same document over MCP, so “write a change brief for this PR” needs no copy-paste.",
      ),
    );
  }

  function paintAll() {
    paintBuilder();
    paintMeta();
    savingsCard.hidden = kind !== "savings";
    gapsCard.hidden = kind !== "gaps";
    if (kind === "savings") paintSavings();
    if (kind === "gaps") paintGaps();
    paintSchedules();
    paintCli();
  }

  paintPicker();
  paintWritten();
  paintAll();
  generate();
  loadSchedules();

  return el(
    "div",
    { class: "view" },
    pageHead(
      "Reports",
      // The claim at the end is not decoration and is asserted by the drive:
      // this is the one page whose whole job is turning a local record into a
      // file for somebody else, so where that file is built and where it goes
      // are the first things a reader needs to know.
      "Turn the ledger, the index and the graph into a file someone else can read — a saving number for finance, an access record for audit, a blast radius for a pull request. Generated locally and exported as a file you own. Nothing leaves the machine.",
    ),
    picker,
    el(
      "div",
      { class: "grid two report-row" },
      builder,
      el(
        "section",
        { class: "card report-preview-card" },
        el(
          "div",
          { class: "report-bar" },
          previewName,
          el("span", { class: "spacer" }),
          copy,
          exportButton,
        ),
        previewBody,
        el("div", { class: "foot" }, previewMeta, el("span", { class: "spacer" }), saveButton),
      ),
    ),
    savingsCard,
    gapsCard,
    writtenCard,
    el("div", { class: "grid two report-row" }, scheduleCard, cliCard),
    /* What the design has no place for, after everything it does place. Both
     * lines are facts about this build rather than about the design, which is
     * why they are here and not woven into a card above. */
    el(
      "section",
      { class: "card pad report-extras" },
      el("span", { class: "card-title", text: "About this build" }),
      el("p", {
        class: "subtitle",
        text:
          "PDF is written by the binary itself — the same blocks as the other four formats, " +
          "typeset on US Letter. Export and Save to disk hand you those bytes unchanged.",
      }),
      el("p", {
        class: "subtitle",
        text:
          "Savings, index health and knowledge gaps count over a store's whole history and " +
          "have no date to narrow by, so the Window chips are inert while one of them is chosen.",
      }),
    ),
  );
}

/* Cloud: a page about a service this binary does not talk to.
 *
 * It is here because the portal is the whole product and a reader should be
 * able to find out what the hosted option is without leaving it — and it is
 * only the not-connected state, because this release ships no cloud client.
 * There is no `semlith cloud` command, no token store and no code path that
 * opens a socket: the command blocks below are text to copy, and what they
 * will do arrives in a later release. */
const CLOUD_ROWS = [
  [
    "One URL for cloud agents",
    "Claude Code cloud sessions, routines and CI cannot reach a laptop; they can reach an org store.",
  ],
  [
    "The whole organisation in one index",
    "Cross-repository paths, and documents beside code.",
  ],
  [
    "A pull-request check that states what the graph proves",
    "With no model and no guess.",
  ],
];

async function cloudView() {
  return el(
    "div",
    // Capped at the design's measure. The page is three paragraphs and two
    // commands; run to 1 400px it reads as a wall rather than as a page.
    { class: "view cloud-page" },
    pageHead("Cloud", null, { pill: pill("not connected", null) }),
    el("p", {
      class: "subtitle",
      text: "Semlith Cloud is one hosted store for a whole organisation: every connected repository a root of it, indexed on push, served over MCP to any agent, with a pull-request impact check and a team ledger. This binary works without it. Nothing here contacts a server.",
    }),
    el(
      "div",
      { class: "card pad" },
      // Title over description, as the design stacks them: a reason and its
      // explanation are two things, and running them into one sentence made
      // the three reasons read as one paragraph of marketing.
      el(
        "div",
        { class: "cloud-rows" },
        CLOUD_ROWS.map(([lead, rest]) =>
          el(
            "div",
            { class: "cloud-row" },
            el("span", { class: "lead", text: lead }),
            el("span", { class: "what", text: rest }),
          ),
        ),
      ),
      el("hr", { class: "rule" }),
      el("div", { class: "cloud-block" }, copyField("semlith cloud login"), el("p", {
        class: "subtitle",
        text: "Not in this release. When it arrives it will store an org token under ~/.semlith/ and send it to that host and no other.",
      })),
      el("div", { class: "cloud-block" }, copyField("semlith cloud connect acme"), el("p", {
        class: "subtitle",
        text: "Will add the org's store to this machine's registry as a remote store, listed beside the local ones with a remote badge.",
      })),
      el("p", {
        class: "note",
        text: "This build has no cloud command and opens no connection to any host. The Privacy page's own reading is where to check that rather than take it from here.",
      }),
      // The v4 design closes this page with a link to semlith.com/data. It is
      // named rather than linked, and that is deliberate: this page is served
      // from a binary that opens no socket, `nothing_in_the_portal_points_at_
      // another_origin` is the test that keeps it that way, and a link the
      // reader cannot follow with the cable out is worse than an address they
      // can type when they have a network.
      el(
        "div",
        { class: "cloud-foot" },
        el("span", { text: "What the cloud stores and deletes is written up at " }),
        mono("semlith.com/data"),
        el("span", { text: "." }),
      ),
    ),
  );
}

const RENDER = {
  stores: storesView,
  files: filesView,
  index: indexView,
  corpus: corpusView,
  search: searchView,
  agents: agentsView,
  privacy: privacyView,
  doctor: doctorView,
  about: aboutView,
  graph: graphView,
  ledger: ledgerView,
  impact: impactView,
  reports: reportsView,
  cloud: cloudView,
};

function go(id) {
  // On a phone the drawer covers the page it is navigating to, so it closes on
  // the way out. Done here rather than on each nav item because every route
  // into a page goes through this — the rail, the drawer, the brand, the search
  // launcher, the "Index a folder" button and the `/` shortcut.
  if (shell.main && window.matchMedia(NARROW).matches && state.navOpen) {
    setNav(false);
  }
  // Re-render even when the hash is already this page: clicking the nav item
  // for the page you are on should still do something, and no hashchange
  // fires for an unchanged hash.
  if ((location.hash || "").slice(1) === id) render();
  else location.hash = `#${id}`;
}

/** Whether dark is showing, whether by choice or by the OS. */
function isDark() {
  const chosen = document.documentElement.getAttribute("data-theme");
  if (chosen) return chosen === "dark";
  return window.matchMedia("(prefers-color-scheme: dark)").matches;
}

/** The mark, in whichever theme is showing. */
function logoImage(size) {
  return el("img", {
    class: "logo",
    src: isDark() ? "logo-dark.svg" : "logo.svg",
    alt: "",
    width: size,
    height: size,
  });
}

/** The three states the control offers, in the order it cycles them. */
const THEMES = ["system", "light", "dark"];

function theme(next) {
  // "system" is the absence of a choice, which is what the stylesheet's
  // prefers-color-scheme block reads. Before this the first click wrote a
  // choice and there was no way back to following the OS without clearing the
  // site's storage.
  if (next === "system") {
    document.documentElement.removeAttribute("data-theme");
  } else {
    document.documentElement.setAttribute("data-theme", next);
  }
  state.theme = next;
  try {
    if (next === "system") localStorage.removeItem("semlith-theme");
    else localStorage.setItem("semlith-theme", next);
  } catch (_) {
    /* a private window refuses storage; the toggle still works for this run */
  }
  for (const img of document.querySelectorAll("img.logo")) {
    img.src = isDark() ? "logo-dark.svg" : "logo.svg";
  }
  paintThemeButton();
}

/** The control shows where it goes, not where you are. */
function paintThemeButton() {
  const button = shell.themeButton;
  if (!button) return;
  const next = THEMES[(THEMES.indexOf(state.theme || "system") + 1) % THEMES.length];
  const label = {
    system: "Follow the system theme",
    light: "Switch to light",
    dark: "Switch to dark",
  }[next];
  const glyph = { system: ICONS.monitor, light: ICONS.sun, dark: ICONS.moon }[next];
  fill(button, icon(glyph));
  button.setAttribute("aria-label", label);
  button.setAttribute("title", label);
}

// ----------------------------------------------------------------- shell

const shell = {};

function buildShell() {
  const hamburger = el(
    "button",
    {
      class: "hamburger",
      type: "button",
      "aria-label": "Toggle navigation",
      "aria-expanded": String(state.navOpen),
      onclick: () => setNav(!state.navOpen, true),
    },
    icon(ICONS.menu, 17),
  );

  const pageTitle = el("span", { class: "page-title" });

  const themeButton = el("button", {
    class: "icon-button",
    type: "button",
    // system → light → dark → system. Three states rather than two, because
    // "follow the system" is one of them and the two-state toggle could not
    // return to it.
    onclick: () =>
      theme(THEMES[(THEMES.indexOf(state.theme || "system") + 1) % THEMES.length]),
  });
  shell.themeButton = themeButton;
  paintThemeButton();

  const topbar = el(
    "div",
    { class: "topbar" },
    hamburger,
    el(
      "button",
      {
        class: "brand",
        type: "button",
        "aria-label": "Semlith — go to Stores",
        onclick: () => go("stores"),
      },
      logoImage(26),
      el("span", { class: "wordmark", text: "Semlith" }),
    ),
    el("span", { class: "divider-v" }),
    pageTitle,
    // On every page, not only on Index: a run the user started and navigated
    // away from should never be something that happened out of sight.
    el("button", {
      class: "pill good run-count",
      type: "button",
      hidden: true,
      title: "Go to Index",
      onclick: () => go("index"),
    }),
    el("span", { class: "spacer" }),
    el(
      "button",
      {
        class: "search-launcher",
        type: "button",
        "aria-label": "Search the index",
        onclick: () => go("search"),
      },
      icon(ICONS.search, 15),
      el("span", { class: "what", text: "Ask the index a question" }),
      el("span", { class: "key-hint", text: "/" }),
    ),
    themeButton,
  );

  const groups = [];
  for (const view of VIEWS) {
    const last = groups[groups.length - 1];
    if (last && last.label === view.group) last.items.push(view);
    else groups.push({ label: view.group, items: [view] });
  }

  const rail = el(
    "nav",
    { class: "rail", "aria-label": "Sections" },
    groups.map((group, i) =>
      el(
        "div",
        { class: "rail-group" },
        group.items.map((view) =>
          el(
            "button",
            {
              class: "rail-item",
              type: "button",
              "data-view": view.id,
              "aria-label": view.label,
              title: view.label,
              onclick: () => go(view.id),
            },
            icon(NAV_ICONS[view.id], 17),
          ),
        ),
        i < groups.length - 1 ? el("div", { class: "rail-rule" }) : null,
      ),
    ),
    el("span", { class: "spacer" }),
    el("span", { class: "live-dot", title: "semlith start" }),
  );

  const sidebar = el(
    "nav",
    { class: "sidebar", "aria-label": "Sections" },
    groups.map((group) =>
      el(
        "div",
        { class: "nav-group" },
        el("div", { class: "nav-group-label", text: group.label }),
        group.items.map((view) =>
          el(
            "button",
            {
              class: "nav-item",
              type: "button",
              "data-view": view.id,
              onclick: () => go(view.id),
            },
            icon(NAV_ICONS[view.id], 16),
            el("span", { text: view.label }),
          ),
        ),
      ),
    ),
    el("span", { class: "spacer" }),
    el(
      "div",
      { class: "daemon-card" },
      el("div", { class: "who" }, el("span", { class: "live-dot" }), mono("semlith start")),
      // The address with its port: "127.0.0.1" alone does not tell you which
      // of two daemons this tab is looking at.
      el("div", { class: "fact", text: `${location.host} · sole writer` }),
      el("div", { class: "fact", id: "daemon-stores", text: "" }),
    ),
  );

  const scrim = el("button", {
    class: "scrim",
    type: "button",
    "aria-label": "Close navigation",
    hidden: true,
    onclick: () => setNav(false, true),
  });

  const main = el("main", {});
  const body = el("div", { class: "body" }, rail, sidebar, scrim, main);

  Object.assign(shell, { topbar, pageTitle, rail, sidebar, scrim, main, hamburger });
  return el("div", { id: "app" }, topbar, body);
}

/** Open or close the navigation without re-rendering the view inside it. */
function setNav(open, fromUser) {
  const was = state.navOpen;
  state.navOpen = open;
  const narrow = window.matchMedia(NARROW).matches;
  shell.hamburger.setAttribute("aria-expanded", String(open));
  shell.sidebar.hidden = !open;
  // The rail is the wide-screen stand-in for the closed drawer. On a narrow
  // screen CSS hides it outright, so it is only ever a wide-screen concern.
  shell.rail.hidden = open || narrow;
  shell.scrim.hidden = !(open && narrow);

  // Over a page, the drawer is a cover — so the keyboard goes into it when it
  // opens and comes back to the button that opened it when it closes. Only for
  // a deliberate toggle: stealing focus on load or on a resize would be rude.
  if (!fromUser || !narrow || open === was) return;
  if (open) {
    const first = shell.sidebar.querySelector(".nav-item");
    if (first) first.focus();
  } else {
    shell.hamburger.focus();
  }
}

function markCurrent(current) {
  for (const node of document.querySelectorAll("[data-view]")) {
    if (node.getAttribute("data-view") === current) node.setAttribute("aria-current", "page");
    else node.removeAttribute("aria-current");
  }
}

let renderGeneration = 0;

async function render() {
  const current = (location.hash || "#stores").slice(1);
  const root = document.getElementById("root");

  // With no store there is nothing for the navigation to navigate, so the
  // first-run screen is the whole page rather than a view inside the shell.
  // `#welcome` asks for it deliberately, from the sidebar's own link, which is
  // the only way back to it once a store exists.
  if (current === "welcome" || (!state.stores.length && current !== "index" && current !== "about")) {
    fill(root, welcomeView());
    shell.main = null;
    return;
  }

  if (!shell.main || !root.contains(shell.main)) {
    fill(root, buildShell());
    setNav(state.navOpen);
  }

  // Every watcher belongs to the view that registered it, so they go when it
  // does. Without this, leaving a page would leave its refetch running.
  resetLive();
  state.onRuns = null;
  state.onStores = null;

  const view = VIEWS.find((v) => v.id === current) || VIEWS[0];
  shell.pageTitle.textContent = view.title;
  document.title = `Semlith · ${view.title}`;
  markCurrent(view.id);

  paintStoreCount();
  paintRunCount();

  const mine = ++renderGeneration;
  fill(shell.main, loadingView(view.title));
  try {
    const node = await (RENDER[view.id] || RENDER.stores)();
    if (mine !== renderGeneration) return;
    fill(shell.main, node);
  } catch (e) {
    if (mine !== renderGeneration) return;
    fill(shell.main, el("div", { class: "view" }, error(e.message)));
  }
}

async function boot() {
  let saved = null;
  try {
    saved = localStorage.getItem("semlith-theme");
  } catch (_) {
    /* private window */
  }
  // Only an explicit choice stamps the attribute. Without one the stylesheet's
  // prefers-color-scheme block decides, so a machine set to dark is not shown
  // a white page and asked to go and find the toggle.
  if (saved === "dark" || saved === "light") {
    theme(saved);
  } else {
    state.theme = "system";
  }

  wireTips();

  /* The change counters' baseline, read before anything they describe. The
   * first poll used to take it a second after the page had drawn, so a change
   * landing in between — a store deleted while the page loaded — was folded
   * into the baseline and never refetched: the page counted a store that was
   * gone until the next unrelated change. Read first, a change in that gap is
   * one the next poll sees move. */
  await pollChanges();
  await refreshStores();
  // Not awaited: the sidebar's agent count is a detail, and `claude mcp list`
  // behind this route is slow on some machines. It fills itself in.
  api("/api/agents").then(noteAgents).catch(() => {});
  // Same reasoning for whether the ledger is recording: one boolean the
  // sidebar states, read once for the life of the tab because a daemon cannot
  // start recording without being restarted.
  api("/api/about")
    .then((about) => {
      state.ledger = about.ledger !== false;
      paintStoreCount();
    })
    .catch(() => {});
  // Likewise the run count: a page opened while three runs are on should say
  // so, and the number is not worth holding the first paint for.
  refreshRuns().catch(() => {});
  await render();

  // One clock, started once, for the life of the tab. Every live view hangs
  // off it; no view starts a timer of its own.
  startLive();

  window.addEventListener("hashchange", render);

  // A width change moves the drawer between overlay and column. Without this
  // the scrim stayed over the content after a resize until the next navigation.
  window.matchMedia(NARROW).addEventListener("change", (e) => {
    if (shell.main) setNav(!e.matches);
  });

  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && state.navOpen && window.matchMedia(NARROW).matches && shell.main) {
      setNav(false, true);
      return;
    }
    if (e.key !== "/" || e.target.matches("input, textarea, select")) return;
    e.preventDefault();
    if ((location.hash || "").slice(1) === "search") {
      const box = document.querySelector(".search-field input");
      if (box) box.focus();
      return;
    }
    go("search");
  });
}

boot();
