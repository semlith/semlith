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

/** Replace an element's children. */
function fill(node, ...kids) {
  node.replaceChildren();
  for (const kid of kids.flat()) {
    if (kid === null || kid === undefined || kid === false) continue;
    node.append(kid.nodeType ? kid : document.createTextNode(String(kid)));
  }
  return node;
}

async function api(path, options) {
  const response = await fetch(path, { credentials: "same-origin", ...options });
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

function when(unix) {
  if (!unix) return "never";
  const seconds = Math.max(0, Math.floor(Date.now() / 1000) - unix);
  if (seconds < 60) return `${seconds}s ago`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
  return `${Math.floor(seconds / 86400)}d ago`;
}

function clock(unix) {
  return new Date(unix * 1000).toTimeString().slice(0, 8);
}

const SVG_NS = "http://www.w3.org/2000/svg";

/** An inline icon. Decorative: the control around it carries the name. */
function icon(d, size) {
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("width", String(size || 16));
  svg.setAttribute("height", String(size || 16));
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
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
  alert: "M12 9v4|M12 17h.01|M12 4 3 19h18z",
  moon: "M20 14.5A8.5 8.5 0 0 1 9.5 4a8.5 8.5 0 1 0 10.5 10.5z",
  copy: "M9 9h9a1 1 0 0 1 1 1v9a1 1 0 0 1-1 1H9a1 1 0 0 1-1-1v-9a1 1 0 0 1 1-1z|M6 15H5a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1h9a1 1 0 0 1 1 1v1",
};

/* The nav marks, from the design. `|` separates subpaths so one mark can be
 * more than a single stroke. */
const NAV_ICONS = {
  stores: "M12 4l8 4-8 4-8-4 8-4|M4 12l8 4 8-4|M4 16.5l8 4 8-4",
  files: "M6 3h7l5 5v13H6z|M13 3v5h5",
  index: "M4 6h16|M4 12h10|M4 18h13",
  search: "M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14z|M16.2 16.2 20 20",
  languages: "M4 6h9|M8 6v2c0 3-2 5-4 6|M7 11c1 2 3 3 5 3.5|M13 20l4-9 4 9|M14.6 17h4.8",
  graph: "M5 6h4v4H5z|M15 14h4v4h-4z|M9 8h4v8h2",
  impact: "M4 18l5-6 4 3 7-9|M20 6h-4|M20 6v4",
  ledger: "M5 4h11l3 3v13H5z|M9 9h6|M9 13h6|M9 17h4",
  agents: "M9 3h6v5H9z|M12 8v3|M5 11h14v9H5z|M9 15h.01|M15 15h.01",
  privacy: "M12 3l7 3v6c0 4.3-3 7.3-7 9-4-1.7-7-4.7-7-9V6z",
  about: "M12 4a8 8 0 1 0 0 16 8 8 0 0 0 0-16z|M12 11v5|M12 8h.01",
};

/** A button that copies text and says so for a moment.
 *
 * Invisible until the pointer enters the block it belongs to — see `.copy` in
 * the stylesheet — so a page never shows a column of copy buttons competing
 * with the content they copy. */
function copyButton(getText, label) {
  const was = label || "Copy";
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
        fill(button, icon(ICONS.copy, 14));
      }, 1400);
    },
  });
  fill(button, icon(ICONS.copy, 14));
  return button;
}

function copyField(command) {
  return el(
    "div",
    { class: "copyfield" },
    el("code", { class: "text", text: command }),
    copyButton(command),
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
 * `show` takes content and the rectangle to hang it off. It prefers to sit
 * above and left-aligned, and flips or clamps when that would leave the
 * viewport. The Graph canvas uses the same element for its richer card, so
 * there is one implementation rather than two. */
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

  /** `content` is a string or an element; `rect` is a viewport rectangle. */
  show(content, rect, owner) {
    const node = this.ensure();
    this.owner = owner || null;
    fill(node, content);
    node.setAttribute("aria-hidden", "false");
    // Measured after filling and before positioning: the size depends on the
    // text, and a stale measurement puts the flip decision on the wrong side.
    node.dataset.open = "true";
    const box = node.getBoundingClientRect();
    const margin = 8;
    let left = rect.left;
    if (left + box.width > window.innerWidth - margin) {
      left = Math.max(margin, window.innerWidth - margin - box.width);
    }
    let top = rect.top - box.height - 6;
    // Above by default, below when there is no room — the flip the criterion
    // asks for, and the reason this is measured against the viewport.
    if (top < margin) top = rect.bottom + 6;
    node.style.left = `${Math.max(margin, Math.round(left))}px`;
    node.style.top = `${Math.round(top)}px`;
  },

  /** Hang a tip off an element. */
  at(target, content) {
    this.show(content || target.getAttribute("data-tip"), target.getBoundingClientRect(), target);
  },

  /** Hang a tip off a point, for the canvas, which has no elements to hang on. */
  atPoint(x, y, content, owner) {
    this.show(content, { left: x + 12, top: y - 8, bottom: y + 18 }, owner);
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
  const find = (node) => (node && node.closest ? node.closest("[data-tip]") : null);

  document.addEventListener("pointerover", (e) => {
    const target = find(e.target);
    if (target) tip.at(target);
    else if (tip.owner && tip.owner.nodeType) tip.hide();
  });
  document.addEventListener("pointerout", (e) => {
    const target = find(e.target);
    if (target) tip.hide(target);
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
const PER_PAGE = [8, 15, 25, 50];

function dataTable(spec) {
  const columns = spec.columns;
  const view = {
    page: 1,
    perPage: spec.perPage || 15,
    sort: spec.sort || null,
    dir: spec.dir || "asc",
  };
  let rows = spec.rows || [];
  let total = spec.total === undefined ? rows.length : spec.total;

  const headRow = el("tr", {});
  const body = el("tbody", {});
  const foot = el("div", { class: "table-foot" });
  const table = el(
    "table",
    { class: spec.className || null },
    el("thead", {}, headRow),
    body,
  );
  const node = el(
    "div",
    { class: "card grow" },
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
      el("span", { class: "eyebrow", text: "per page" }),
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
        class: "button ghost small",
        type: "button",
        text: "Prev",
        disabled: view.page <= 1,
        onclick: () => {
          view.page -= 1;
          changed();
        },
      }),
      el("span", { class: "meta", text: `page ${n(view.page)} of ${n(pages())}` }),
      el("button", {
        class: "button ghost small",
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

/** A status pill. One shape, and a dot only where it reports a state. */
function pill(text, tone) {
  return el(
    "span",
    { class: tone ? `pill ${tone}` : "pill" },
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
  /** Carried from a Graph selection into Impact, or into Search. */
  pendingSymbol: "",
  pendingQuery: "",
};

/* Pages, in the design's grouping. The ones the roadmap puts in a later
 * release — Inside the index, Reports, License — are deliberately absent
 * rather than stubbed: a nav item that leads nowhere is worse than one that
 * does not exist yet. Everything listed here is free on every tier. */
const VIEWS = [
  { group: "Workspace", id: "stores", label: "Stores", title: "Stores" },
  { group: "Workspace", id: "files", label: "Files", title: "Files" },
  { group: "Workspace", id: "index", label: "Index", title: "Index" },
  { group: "Explore", id: "search", label: "Search", title: "Search" },
  { group: "Explore", id: "graph", label: "Graph", title: "Graph" },
  { group: "Explore", id: "impact", label: "Impact", title: "Impact" },
  { group: "Explore", id: "languages", label: "Languages", title: "Languages" },
  { group: "Operate", id: "agents", label: "Agents", title: "Agents" },
  { group: "Operate", id: "ledger", label: "Ledger", title: "Retrieval ledger" },
  { group: "Operate", id: "privacy", label: "Privacy", title: "Privacy" },
  { group: "About", id: "about", label: "About", title: "About" },
];

/** The sidebar's count, kept with the data it describes rather than with the
 * render that happened to be running when it changed. */
function paintStoreCount() {
  const node = document.getElementById("daemon-stores");
  if (!node) return;
  const many = state.stores.length;
  node.textContent = `${many} store${many === 1 ? "" : "s"}`;
}


// ---------------------------------------------------------------- graph

/* The colours the canvas draws with, read from the stylesheet rather than
 * repeated here: the page is themed by CSS custom properties, and a canvas
 * cannot inherit one. Re-read on every paint so a theme switch is picked up. */
function graphInk() {
  const style = getComputedStyle(document.documentElement);
  const read = (name, fallback) => (style.getPropertyValue(name) || fallback).trim();
  return {
    extracted: read("--green", "#3e9a6e"),
    inferred: read("--amber-ink", "#7a4e0a"),
    selected: read("--accent-edge", "#c97f14"),
    line: read("--line", "#dce3e8"),
    ink: read("--ink", "#1e2a35"),
    muted: read("--muted", "#64778a"),
    panel: read("--panel", "#ffffff"),
  };
}

/* A force layout, written out because the portal may not load a library: the
 * CSP allows only 'self' and every byte is compiled into the binary.
 *
 * ponytail: O(n²) repulsion over every pair each frame. The node budget is 180,
 * so that is ~16k pairs — fine at 60fps. If the budget ever rises past a few
 * hundred, this wants a quadtree, not a faster loop.
 */
function layout(nodes, edges, width, height) {
  const cx = width / 2;
  const cy = height / 2;
  // Repulsion has to grow with the crowd. A constant that reads well at twenty
  // nodes collapses a hundred into one ball, which is the hairball every graph
  // view is accused of being.
  const repulsion = 2200 + nodes.length * 90;
  const pad = 26;
  // The rest length grows with the crowd: forty nodes at ninety pixels apart
  // need more room than eight do, and a constant here is what turns a dense
  // neighbourhood into a knot.
  const rest = 70 + nodes.length * 1.6;
  // How many edges each node carries, so the springs can be shared out. A hub
  // with forty edges would otherwise be dragged to the centre by forty pulls
  // while a leaf feels one, and every graph would collapse to its hubs.
  const load = nodes.map(() => 1);
  for (const edge of edges) {
    if (load[edge.from] !== undefined) load[edge.from]++;
    if (load[edge.to] !== undefined) load[edge.to]++;
  }
  // Seeded on a circle rather than at random, so the same graph draws the same
  // way twice. A layout that reshuffles on every visit is one nobody can learn.
  nodes.forEach((node, i) => {
    const angle = (i / Math.max(nodes.length, 1)) * Math.PI * 2;
    const radius = Math.min(width, height) * 0.32;
    node.x = cx + Math.cos(angle) * radius;
    node.y = cy + Math.sin(angle) * radius;
    node.vx = 0;
    node.vy = 0;
  });

  return function step(alpha) {
    for (let i = 0; i < nodes.length; i++) {
      const a = nodes[i];
      for (let j = i + 1; j < nodes.length; j++) {
        const b = nodes[j];
        let dx = a.x - b.x;
        let dy = a.y - b.y;
        let d2 = dx * dx + dy * dy;
        if (d2 < 0.01) {
          // Exactly coincident nodes have no direction to separate along.
          dx = (i % 2 ? 1 : -1) * 0.5;
          dy = 0.5;
          d2 = 0.5;
        }
        const force = (repulsion / d2) * alpha;
        const d = Math.sqrt(d2);
        a.vx += (dx / d) * force;
        a.vy += (dy / d) * force;
        b.vx -= (dx / d) * force;
        b.vy -= (dy / d) * force;
      }
    }
    for (const edge of edges) {
      const a = nodes[edge.from];
      const b = nodes[edge.to];
      if (!a || !b) continue;
      const dx = b.x - a.x;
      const dy = b.y - a.y;
      const d = Math.sqrt(dx * dx + dy * dy) || 1;
      const pull = ((d - rest) / d) * 0.06 * alpha;
      a.vx += (dx * pull) / Math.sqrt(load[edge.from]);
      a.vy += (dy * pull) / Math.sqrt(load[edge.from]);
      b.vx -= (dx * pull) / Math.sqrt(load[edge.to]);
      b.vy -= (dy * pull) / Math.sqrt(load[edge.to]);
    }
    for (const node of nodes) {
      if (node.held) continue;
      node.vx += (cx - node.x) * 0.0016 * alpha;
      node.vy += (cy - node.y) * 0.0016 * alpha;
      node.x += node.vx;
      node.y += node.vy;
      node.vx *= 0.84;
      node.vy *= 0.84;
      // Kept inside the frame. A node that drifts off-canvas is a node nobody
      // can click, and the pan control is for choosing a view, not for going
      // to fetch something that escaped.
      node.x = Math.min(width - pad, Math.max(pad, node.x));
      node.y = Math.min(height - pad, Math.max(pad, node.y));
    }
  };
}

/** The last two segments of a path: enough to recognise, short enough to read.
 * The whole path is on the element's title, which is where someone goes when
 * two files share a name. */
function shortPath(path) {
  const parts = String(path).split("/").filter(Boolean);
  return parts.slice(-2).join("/") || path;
}

/** True when the viewer has asked for less movement. */
function stillness() {
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/* A pannable, zoomable canvas of nodes and edges.
 *
 * Returns the element plus `draw(data)` and `stop()`. The caller owns the data;
 * this owns the pixels. */
function graphCanvas(onPick) {
  const canvas = el("canvas", { class: "graph-canvas" });
  const wrap = el("div", { class: "graph-stage" }, canvas);
  const view = { x: 0, y: 0, scale: 1 };
  let nodes = [];
  let edges = [];
  let selected = null;
  let step = null;
  let alpha = 0;
  let frame = null;
  let running = true;
  let dragging = null;
  let panning = null;

  function size() {
    const ratio = window.devicePixelRatio || 1;
    const box = wrap.getBoundingClientRect();
    const w = Math.max(box.width, 320);
    const h = Math.max(box.height, 320);
    canvas.width = Math.round(w * ratio);
    canvas.height = Math.round(h * ratio);
    return { w, h, ratio };
  }

  function paint() {
    const { w, h, ratio } = size();
    const ink = graphInk();
    const ctx = canvas.getContext("2d");
    void h;
    ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
    ctx.clearRect(0, 0, w, h);
    ctx.save();
    ctx.translate(view.x, view.y);
    ctx.scale(view.scale, view.scale);

    for (const edge of edges) {
      const a = nodes[edge.from];
      const b = nodes[edge.to];
      if (!a || !b) continue;
      ctx.beginPath();
      ctx.moveTo(a.x, a.y);
      ctx.lineTo(b.x, b.y);
      ctx.strokeStyle = edge.confidence === "extracted" ? ink.extracted : ink.inferred;
      // An inferred edge is drawn dashed as well as coloured: colour alone is
      // not a distinction everyone can see.
      ctx.setLineDash(edge.confidence === "extracted" ? [] : [4, 3]);
      ctx.lineWidth = edge.confidence === "extracted" ? 1.2 : 1;
      ctx.globalAlpha = selected && edge.from !== selected && edge.to !== selected ? 0.22 : 0.75;
      ctx.stroke();
    }
    ctx.setLineDash([]);
    ctx.globalAlpha = 1;

    for (let i = 0; i < nodes.length; i++) {
      const node = nodes[i];
      const on = i === selected;
      ctx.beginPath();
      ctx.arc(node.x, node.y, on ? 8 : 5.5, 0, Math.PI * 2);
      ctx.fillStyle = on ? ink.selected : ink.panel;
      ctx.strokeStyle = on ? ink.selected : ink.muted;
      ctx.lineWidth = on ? 2.5 : 1.4;
      ctx.fill();
      ctx.stroke();
      // Labels only where they can be read. Every node labelled at once is a
      // grey haze that makes the shape harder to see rather than easier, so
      // this draws the selection, whatever it touches, and the busiest few —
      // and everything only once the view is zoomed in far enough to have room.
      // On a phone the frame is ~350px wide and a dozen labels is a smear, so
      // there the selection and its neighbours carry names and nothing else
      // does until the reader zooms in.
      const roomy = w > 520;
      if (on || node.near || (roomy && node.busy && view.scale > 0.75) || view.scale > 2) {
        ctx.font = `${on ? 600 : 400} 11px "IBM Plex Mono", ui-monospace, monospace`;
        ctx.fillStyle = on ? ink.ink : ink.muted;
        ctx.textAlign = "center";
        // Test names run to sixty characters and would cover their
        // neighbours. The full name is in the rail the moment a node is
        // picked, so the canvas shows as much as fits.
        const label = node.name.length > 22 ? `${node.name.slice(0, 21)}…` : node.name;
        ctx.fillText(label, node.x, node.y - (on ? 13 : 10));
      }
    }
    ctx.restore();
  }

  function tick() {
    if (step && alpha > 0.005) {
      step(alpha);
      alpha *= 0.97;
    }
    paint();
    frame = running ? requestAnimationFrame(tick) : null;
  }

  function at(event) {
    const box = canvas.getBoundingClientRect();
    return {
      x: (event.clientX - box.left - view.x) / view.scale,
      y: (event.clientY - box.top - view.y) / view.scale,
    };
  }

  function nearest(point) {
    let best = null;
    let bestD = 14 / view.scale;
    nodes.forEach((node, i) => {
      const d = Math.hypot(node.x - point.x, node.y - point.y);
      if (d < bestD) {
        bestD = d;
        best = i;
      }
    });
    return best;
  }

  canvas.addEventListener("pointerdown", (event) => {
    const point = at(event);
    const hit = nearest(point);
    canvas.setPointerCapture(event.pointerId);
    if (hit === null) {
      panning = { x: event.clientX - view.x, y: event.clientY - view.y };
      return;
    }
    selected = hit;
    for (const node of nodes) node.near = false;
    for (const edge of edges) {
      if (edge.from === hit && nodes[edge.to]) nodes[edge.to].near = true;
      if (edge.to === hit && nodes[edge.from]) nodes[edge.from].near = true;
    }
    nodes[hit].held = true;
    dragging = hit;
    alpha = Math.max(alpha, 0.25);
    if (onPick) onPick(nodes[hit]);
    paint();
  });

  canvas.addEventListener("pointermove", (event) => {
    if (dragging !== null) {
      const point = at(event);
      nodes[dragging].x = point.x;
      nodes[dragging].y = point.y;
      paint();
    } else if (panning) {
      view.x = event.clientX - panning.x;
      view.y = event.clientY - panning.y;
      paint();
    }
  });

  const release = () => {
    if (dragging !== null) nodes[dragging].held = false;
    dragging = null;
    panning = null;
  };
  canvas.addEventListener("pointerup", release);
  canvas.addEventListener("pointercancel", release);

  canvas.addEventListener("wheel", (event) => {
    event.preventDefault();
    const box = canvas.getBoundingClientRect();
    const px = event.clientX - box.left;
    const py = event.clientY - box.top;
    const factor = Math.exp(-event.deltaY * 0.0015);
    const next = Math.min(3, Math.max(0.3, view.scale * factor));
    view.x = px - ((px - view.x) / view.scale) * next;
    view.y = py - ((py - view.y) / view.scale) * next;
    view.scale = next;
    paint();
  });

  return {
    node: wrap,
    draw(data) {
      const { w, h } = size();
      nodes = (data.nodes || []).map((row) => ({ ...row, degree: 0 }));
      edges = data.edges || [];
      for (const edge of edges) {
        if (nodes[edge.from]) nodes[edge.from].degree++;
        if (nodes[edge.to]) nodes[edge.to].degree++;
      }
      // The dozen busiest nodes carry a label at moderate zoom: they are the
      // ones a reader is orienting by.
      const ranked = [...nodes].sort((a, b) => b.degree - a.degree).slice(0, 12);
      for (const node of ranked) node.busy = true;
      selected = null;
      step = layout(nodes, edges, w, h);
      // Under reduced motion the layout is solved before the first paint, so
      // the page arrives settled instead of animating into place.
      if (stillness()) {
        for (let i = 0; i < 420; i++) step(1);
        step = null;
        alpha = 0;
      } else {
        alpha = 1;
      }
      paint();
    },
    /** Select a node by name, as though it had been clicked. */
    pick(name) {
      const index = nodes.findIndex((node) => node.name === name);
      if (index < 0) return false;
      selected = index;
      for (const node of nodes) node.near = false;
      for (const edge of edges) {
        if (edge.from === index && nodes[edge.to]) nodes[edge.to].near = true;
        if (edge.to === index && nodes[edge.from]) nodes[edge.from].near = true;
      }
      paint();
      if (onPick) onPick(nodes[index]);
      return true;
    },
    running: () => running,
    toggle() {
      running = !running;
      if (running) {
        alpha = Math.max(alpha, 0.35);
        tick();
      } else if (frame) {
        cancelAnimationFrame(frame);
        frame = null;
      }
      return running;
    },
    start() {
      running = !stillness();
      if (running) tick();
      else paint();
    },
    stop() {
      running = false;
      if (frame) cancelAnimationFrame(frame);
      frame = null;
    },
    repaint: paint,
  };
}

async function graphView() {
  await refreshStores();

  const chosen = new Set();
  const meta = el("span", { class: "graph-meta" });
  const rail = el("div", { class: "graph-rail" });

  const canvas = graphCanvas((node) => select(node));

  const pause = el("button", {
    class: "button secondary small",
    type: "button",
    text: "Pause",
    onclick: (e) => {
      const on = canvas.toggle();
      e.currentTarget.textContent = on ? "Pause" : "Resume";
    },
  });

  /* The label has to come from what the canvas is actually doing, not from a
   * second reading of the media query: under reduced motion `start` never
   * begins a loop, and a button reading "Pause" beside a still picture is a
   * lie about which of the two is in charge. */
  function paintPause() {
    pause.textContent = canvas.running() ? "Pause" : "Settled";
  }

  function blank(message) {
    return fill(
      rail,
      el("div", { class: "graph-selected" }, el("div", { class: "rail-hint", text: message })),
    );
  }

  function ends(title, list, absent) {
    return el(
      "div",
      { class: "rail-group" },
      el("h3", { text: title }),
      list.length
        ? el(
            "ul",
            { class: "rail-list" },
            list.map((end) =>
              el(
                "li",
                {},
                el("a", {
                  href: `#graph?name=${encodeURIComponent(end.name)}`,
                  text: end.name,
                  onclick: (e) => {
                    e.preventDefault();
                    focus(end.name);
                  },
                }),
                el("span", {
                  class: `conf ${end.confidence}`,
                  text: end.confidence,
                  title:
                    end.confidence === "extracted"
                      ? "Resolved through an import in the source file."
                      : "Matched by name. Two functions can share one.",
                }),
                el("span", { class: "via", text: end.kind }),
              ),
            ),
          )
        : el("div", { class: "rail-hint", text: absent }),
    );
  }

  async function select(node) {
    fill(rail, el("div", { class: "rail-hint", text: "Loading…" }));
    let data;
    try {
      data = await api(`/api/neighbors?name=${encodeURIComponent(node.name)}`);
    } catch (e) {
      return fill(rail, error(e.message));
    }
    const callers = data.callers.map((e) => ({
      name: e.name,
      kind: e.kind,
      confidence: e.confidence,
    }));
    const callees = data.callees.map((e) => ({
      name: e.name,
      kind: e.kind,
      confidence: e.confidence,
    }));

    fill(
      rail,
      el(
        "div",
        { class: "graph-selected" },
        el("h2", { class: "sym", text: node.name }),
        el("div", {
          class: "loc",
          title: node.path,
          text: `${shortPath(node.path)}:${node.start_line}-${node.end_line}`,
        }),
        el(
          "div",
          { class: "chips" },
          el("span", { class: "chip static", text: node.kind }),
          node.store ? el("span", { class: "chip static", text: node.store }) : null,
          el("span", {
            class: "chip static",
            text: `${Math.max(node.end_line - node.start_line + 1, 1)} lines`,
          }),
        ),
      ),
      ends("Callers", callers, "Nothing in the graph calls this."),
      ends("Callees", callees, "A leaf, as far as the extracted edges go."),
      el(
        "div",
        { class: "rail-actions" },
        el("a", {
          class: "button secondary small",
          href: `#search`,
          text: "Chunks it lives in",
          onclick: (e) => {
            e.preventDefault();
            state.pendingQuery = node.name;
            go("search");
          },
        }),
        el("a", {
          class: "button small",
          href: `#impact`,
          text: "Blast radius",
          onclick: (e) => {
            e.preventDefault();
            state.pendingSymbol = node.name;
            go("impact");
          },
        }),
      ),
    );
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
      return fill(rail, error(e.message));
    }
    if (!data.nodes.length) {
      meta.textContent = "";
      canvas.draw({ nodes: [], edges: [] });
      return blank(
        data.total === 0
          ? "This store has no symbols yet. The graph is built as files are indexed — run Index once, or leave the daemon watching."
          : "Nothing in this scope. Clear the filters, or pick a store.",
      );
    }
    meta.textContent =
      data.shown < data.total
        ? `${n(data.shown)} of ${n(data.total)} symbols · ${n(data.edges.length)} edges drawn`
        : `${n(data.shown)} symbols · ${n(data.edges.length)} edges`;
    canvas.draw(data);
    // A focused view arrives with its centre chosen, so the rail says something
    // before the first click rather than asking for one.
    const centre = params && params.name;
    if (centre && canvas.pick(centre)) return;
    blank("Pick a node to see what calls it and what it calls.");
  }

  function focus(name) {
    load({ name, limit: "45" });
  }

  const scopeInput = el("input", {
    type: "search",
    placeholder: "Scope to a path, or find a symbol",
    onkeydown: (e) => {
      if (e.key !== "Enter") return;
      const value = e.currentTarget.value.trim();
      if (!value) return load({});
      // A path fragment scopes; anything else is read as a symbol to centre on.
      load(value.includes("/") || value.includes(".") ? { path: value } : { name: value });
    },
  });

  const storeChips = state.stores.map((store) =>
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
    { class: "view graph-page" },
    pageHead(
      "Graph",
      "Edges are re-extracted on the same pass that re-embeds a file. Never a stale build artifact.",
    ),
    el(
      "div",
      { class: "graph-controls" },
      el("div", { class: "graph-scope" }, icon(ICONS.search, 16), labelled("graph-scope", "Scope the graph", scopeInput)),
      el("div", { class: "filters" }, storeChips.length > 1 ? storeChips : null),
      el("span", { class: "spacer" }),
      pause,
    ),
    el(
      "div",
      { class: "graph-body grow" },
      el(
        "div",
        { class: "graph-frame" },
        canvas.node,
        el(
          "div",
          { class: "graph-foot" },
          el(
            "div",
            { class: "graph-legend" },
            el("span", { class: "key extracted" }, el("i", {}), "extracted"),
            el("span", { class: "key inferred" }, el("i", {}), "inferred"),
            el("span", { class: "key selected" }, el("i", {}), "selected"),
          ),
          meta,
        ),
      ),
      rail,
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
    // once. The whole store drawn at once is honest and unreadable — hundreds
    // of edges among the hubs is what the corpus actually looks like — and a
    // reader arriving at a knot learns nothing. One symbol and what touches it
    // is the view the rest of the page is about, and the scope box widens it.
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

// --------------------------------------------------------------- impact

/** Reverse reachability drawn as rings, one ring per hop. */
function ringCanvas() {
  const canvas = el("canvas", { class: "graph-canvas" });
  const wrap = el("div", { class: "rings" }, canvas);
  let rows = [];
  let centre = "";

  function paint() {
    const ratio = window.devicePixelRatio || 1;
    const box = wrap.getBoundingClientRect();
    const w = Math.max(box.width, 240);
    const h = Math.max(box.height, 240);
    canvas.width = Math.round(w * ratio);
    canvas.height = Math.round(h * ratio);
    const ink = graphInk();
    const ctx = canvas.getContext("2d");
    ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
    ctx.clearRect(0, 0, w, h);

    const cx = w / 2;
    const cy = h / 2;
    const hops = Math.max(1, ...rows.map((r) => r.hops));
    const gap = Math.min(w, h) / 2 / (hops + 1);

    for (let hop = 1; hop <= hops; hop++) {
      ctx.beginPath();
      ctx.arc(cx, cy, gap * hop, 0, Math.PI * 2);
      ctx.strokeStyle = ink.line;
      ctx.lineWidth = 1;
      ctx.stroke();
    }

    const byHop = new Map();
    for (const row of rows) {
      if (!byHop.has(row.hops)) byHop.set(row.hops, []);
      byHop.get(row.hops).push(row);
    }
    for (const [hop, list] of byHop) {
      list.forEach((row, i) => {
        const angle = (i / list.length) * Math.PI * 2 - Math.PI / 2;
        const x = cx + Math.cos(angle) * gap * hop;
        const y = cy + Math.sin(angle) * gap * hop;
        ctx.beginPath();
        ctx.moveTo(cx, cy);
        ctx.lineTo(x, y);
        ctx.strokeStyle = row.confidence === "extracted" ? ink.extracted : ink.inferred;
        ctx.setLineDash(row.confidence === "extracted" ? [] : [4, 3]);
        ctx.globalAlpha = 0.5;
        ctx.stroke();
        ctx.setLineDash([]);
        ctx.globalAlpha = 1;

        ctx.beginPath();
        ctx.arc(x, y, 4.5, 0, Math.PI * 2);
        ctx.fillStyle = ink.panel;
        ctx.strokeStyle = row.confidence === "extracted" ? ink.extracted : ink.inferred;
        ctx.lineWidth = 1.5;
        ctx.fill();
        ctx.stroke();
      });
    }

    ctx.beginPath();
    ctx.arc(cx, cy, 9, 0, Math.PI * 2);
    ctx.fillStyle = ink.selected;
    ctx.fill();
    ctx.font = '600 11px "IBM Plex Mono", ui-monospace, monospace';
    ctx.fillStyle = ink.ink;
    ctx.textAlign = "center";
    ctx.fillText(centre, cx, cy - 15);
  }

  return {
    node: wrap,
    draw(name, list) {
      centre = name;
      rows = list;
      paint();
    },
  };
}

async function impactView() {
  const summary = el("div", { class: "impact-summary" });
  const table = el("div", { class: "impact-table" });
  const rings = ringCanvas();
  const pathOut = el("div", { class: "path-steps" });

  const name = el("input", {
    type: "search",
    placeholder: "A symbol you are about to change",
    value: state.pendingSymbol || "",
    onkeydown: (e) => {
      if (e.key === "Enter") run();
    },
  });
  state.pendingSymbol = "";

  const depth = el("select", { onchange: () => run() }, [1, 2, 3, 4, 5].map((d) =>
    el("option", { value: String(d), selected: d === 3, text: `${d} hop${d === 1 ? "" : "s"}` }),
  ));

  async function run() {
    const symbol = name.value.trim();
    if (!symbol) {
      fill(summary, empty("Name a symbol. Its blast radius is what reaches it, not what it reaches."));
      fill(table);
      fill(pathOut);
      rings.draw("", []);
      return;
    }
    fill(summary, el("div", { class: "rail-hint", text: "Walking the edges…" }));
    let data;
    try {
      data = await api(`/api/impact?name=${encodeURIComponent(symbol)}&depth=${depth.value}`);
    } catch (e) {
      return fill(summary, error(e.message));
    }

    const rows = data.reached || [];
    const inferred = rows.filter((r) => r.confidence !== "extracted").length;
    const files = new Set(rows.map((r) => r.path)).size;

    fill(
      summary,
      el(
        "div",
        { class: "impact-head" },
        el("span", { class: "label", text: "Changing" }),
        el("span", { class: "sym", text: symbol }),
        el("span", { class: "chip static amber", text: `${data.depth} hop${data.depth === 1 ? "" : "s"}` }),
      ),
      el(
        "div",
        { class: "stat-row" },
        stat("Symbols reached", n(rows.length)),
        stat("Files touched", n(files)),
        stat("Matched by name", n(inferred), inferred ? "not resolved through an import" : "every edge resolved"),
      ),
      data.truncated
        ? el("div", { class: "rail-hint", text: "Stopped at the node budget; the real radius is larger." })
        : null,
    );

    if (!rows.length) {
      fill(table, empty("Nothing in the graph reaches this. Either it is a leaf, or its callers are in a language that carries no edges."));
      rings.draw(symbol, []);
      return;
    }

    fill(
      table,
      el(
        "table",
        {},
        el(
          "thead",
          {},
          el(
            "tr",
            {},
            el("th", { text: "Reached symbol" }),
            el("th", { text: "Via" }),
            el("th", { text: "Hops" }),
            el("th", { text: "Where" }),
          ),
        ),
        el(
          "tbody",
          {},
          rows.map((row) =>
            el(
              "tr",
              {},
              el("td", { class: "mono", text: row.name }),
              el(
                "td",
                {},
                el("span", { text: row.via }),
                el("span", { class: `conf ${row.confidence}`, text: row.confidence }),
              ),
              el("td", { text: String(row.hops) }),
              el("td", {
                class: "where",
                title: row.path,
                text: `${shortPath(row.path)}:${row.start_line}`,
              }),
            ),
          ),
        ),
      ),
    );
    rings.draw(symbol, rows);
  }

  // ---- path finder
  const from = el("input", { type: "search", placeholder: "from" });
  const to = el("input", { type: "search", placeholder: "to" });

  async function findPath() {
    const a = from.value.trim();
    const b = to.value.trim();
    if (!a || !b) {
      return fill(pathOut, empty("Name both ends."));
    }
    fill(pathOut, el("div", { class: "rail-hint", text: "Searching…" }));
    let data;
    try {
      data = await api(`/api/path?from=${encodeURIComponent(a)}&to=${encodeURIComponent(b)}`);
    } catch (e) {
      return fill(pathOut, error(e.message));
    }
    const steps = data.path;
    if (!steps) {
      return fill(
        pathOut,
        empty(`No chain from ${a} to ${b}. They may be unconnected, or connected further than six hops.`),
      );
    }
    if (!steps.length) {
      return fill(pathOut, empty(`${a} is ${b}.`));
    }
    fill(
      pathOut,
      el(
        "ol",
        { class: "chain" },
        steps.map((step) =>
          el(
            "li",
            {},
            el("span", { class: "mono", text: step.from }),
            el("span", { class: "edge", text: step.kind }),
            el("span", { class: "mono", text: step.to }),
            el("span", { class: `conf ${step.confidence}`, text: step.confidence }),
          ),
        ),
      ),
      el("div", {
        class: "rail-hint",
        text: `${steps.length} hop${steps.length === 1 ? "" : "s"}, ${
          steps.every((s) => s.confidence === "extracted") ? "all extracted" : "some matched by name"
        }.`,
      }),
    );
  }

  from.addEventListener("keydown", (e) => e.key === "Enter" && findPath());
  to.addEventListener("keydown", (e) => e.key === "Enter" && findPath());

  const page = el(
    "div",
    { class: "view impact-page" },
    el(
      "div",
      { class: "impact-title" },
      pageHead(
        "Impact",
        "Reverse reachability. What breaks if this changes — before the edit, not after the test run.",
      ),
      el("span", { class: "tool-name", text: "semlith_impact" }),
    ),
    el(
      "div",
      { class: "impact-controls" },
      el("div", { class: "graph-scope" }, icon(ICONS.search, 16), labelled("impact-name", "Symbol", name)),
      labelled("impact-depth", "Hop depth", depth),
      el("button", { class: "button small", type: "button", text: "Show", onclick: run }),
    ),
    summary,
    el("div", { class: "impact-body" }, table, rings.node),
    el(
      "div",
      { class: "path-finder" },
      el("h2", { text: "Path finder" }),
      el(
        "div",
        { class: "path-controls" },
        labelled("path-from", "From symbol", from),
        icon(ICONS.up, 16),
        labelled("path-to", "To symbol", to),
        el("button", { class: "button secondary small", type: "button", text: "Find", onclick: findPath }),
      ),
      pathOut,
    ),
  );

  fill(pathOut, empty("Name two symbols to see the shortest chain of edges between them."));
  setTimeout(() => {
    if (name.value) run();
    else fill(summary, empty("Name a symbol. Its blast radius is what reaches it, not what it reaches."));
  }, 0);
  return page;
}

/** One figure with its label, as the design draws them. */
function stat(label, value, note) {
  return el(
    "div",
    { class: "stat" },
    el("div", { class: "stat-label", text: label }),
    el("div", { class: "stat-value", text: value }),
    note ? el("div", { class: "stat-note", text: note }) : null,
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
  return el(
    "div",
    { class: "view ledger-page" },
    pageHead(
      "Retrieval ledger",
      "Every query an agent ran, recorded locally. The honest token number, a debugging trail, and an audit record that never left the machine.",
      { pill: pill(on ? "recording · opt-in" : "not recording", on ? "on" : null) },
    ),
    el(
      "div",
      { class: "stat-row wide" },
      stat("Queries recorded", n(data.queries), `${n(data.clients)} client${data.clients === 1 ? "" : "s"}`),
      stat("Excerpt tokens", n(data.excerpt_tokens), "what the agents actually read"),
      stat("Whole-file tokens", n(data.whole_file_tokens), "what a grep loop would have cost"),
      stat(
        "Measured ratio",
        data.ratio ? `${data.ratio.toFixed(1)}×` : "—",
        data.ratio ? "not a marketing claim" : "needs a recorded query",
      ),
    ),
    el(
      "div",
      { class: "scroller" },
    copyField("semlith ledger --last 20"),
    el("div", {
      class: "rail-hint",
      text: "Prints the ledger on the command line. Nothing here needs a key.",
    }),
    on
      ? null
      : el(
          "div",
          { class: "notice" },
          el("div", { class: "what", text: "Recording is off" }),
          el("div", {
            text: "Start the daemon with --ledger to record what your agents retrieve. Nothing is sent anywhere; the rows live in the store beside the chunks.",
          }),
        ),
    el(
      "div",
      { class: "note" },
      el("div", { class: "what", text: "Per-session views arrive with the paid release" }),
      el("div", {
        text: "Recording and the semlith ledger dump are free, permanently. The per-session table, the filters and CSV/JSON export are what 0.13.0 adds on top — they will not lock anything that works today.",
      }),
    ),
    ),
  );
}

/** Read the store list into `state`, so every view agrees on how many exist. */
async function refreshStores() {
  try {
    const data = await api("/api/stores");
    state.stores = data.stores || [];
    paintStoreCount();
  } catch (_) {
    // A failed refresh must not empty the list: `state.stores.length` decides
    // whether the app or the first-run screen is shown, and a dropped request
    // is not the same as having no stores.
  }
  return state.stores;
}

// ---------------------------------------------------------------- stores

async function storesView() {
  const stores = await refreshStores();

  if (!stores.length) {
    return el(
      "div",
      { class: "view" },
      pageHead("Stores", "Everything indexed on this machine. Nothing leaves it."),
      empty("No store is open. Index a folder and it appears here."),
    );
  }

  const totals = stores.reduce(
    (sum, s) => ({
      files: sum.files + s.files,
      chunks: sum.chunks + s.chunks,
      bytes: sum.bytes + s.bytes,
      watching: sum.watching + (s.watching ? 1 : 0),
    }),
    { files: 0, chunks: 0, bytes: 0, watching: 0 },
  );

  const stat = (key, value, sub) =>
    el(
      "div",
      { class: "stat" },
      el("span", { class: "eyebrow", text: key }),
      el("span", { class: "value", text: value }),
      el("span", { class: "sub", text: sub }),
    );

  const feed = stores
    .flatMap((s) => (s.events || []).map((e) => ({ at: e.at, text: e.text })))
    .sort((a, b) => b.at - a.at)
    .slice(0, 40);

  return el(
    "div",
    { class: "view" },
    pageHead("Stores", "Everything indexed on this machine. Nothing leaves it.", {
      actions: el(
        "button",
        { class: "button", type: "button", onclick: () => go("index") },
        icon(ICONS.plus),
        "Index a folder",
      ),
    }),
    el(
      "div",
      { class: "strip" },
      stat("Stores", n(stores.length), `${totals.watching} being watched`),
      stat("Files", n(totals.files), "readable by a person"),
      stat("Chunks", n(totals.chunks), "embedded and searchable"),
      stat("On disk", bytes(totals.bytes), "vectors and text together"),
    ),
    el(
      "div",
      { class: "scroller" },
    el(
      "div",
      { class: "card" },
      el(
        "div",
        { class: "table-wrap" },
        el(
          "table",
          { class: "w-stores" },
          el(
            "thead",
            {},
            el(
              "tr",
              {},
              el("th", { text: "Store" }),
              el("th", { text: "Roots" }),
              el("th", { class: "num", text: "Files" }),
              el("th", { class: "num", text: "Chunks" }),
              el("th", { text: "Last write" }),
              el("th", { text: "" }),
            ),
          ),
          el(
            "tbody",
            {},
            stores.map((s) =>
              el(
                "tr",
                {},
                el(
                  "td",
                  {},
                  el("div", { class: "name", text: s.name }),
                  el("div", { class: "meta", "data-tip": s.dir, text: s.dir }),
                ),
                el(
                  "td",
                  { class: "meta" },
                  (s.roots || []).length
                    ? s.roots.map((r) =>
                        el("div", {
                          class: r.present ? "" : "gone",
                          text: r.present ? r.path : `${r.path} (missing)`,
                        }),
                      )
                    : "—",
                ),
                el("td", { class: "num", text: n(s.files) }),
                el("td", { class: "num", text: n(s.chunks) }),
                el(
                  "td",
                  {},
                  pill(s.watching ? when(s.last_write) : "not watching", s.watching ? "good" : "warn"),
                ),
                el(
                  "td",
                  {},
                  el("button", {
                    class: "button ghost small",
                    type: "button",
                    text: "Files",
                    onclick: () => go("files"),
                  }),
                ),
              ),
            ),
          ),
        ),
      ),
    ),
    el(
      "div",
      { class: "grid" },
      el(
        "div",
        { class: "card pad" },
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
              feed.map((e) =>
                el(
                  "div",
                  { class: "row" },
                  el("span", { class: "at", text: clock(e.at) }),
                  el("span", { class: "what", text: e.text }),
                ),
              ),
            )
          : empty("Nothing has changed on disk since the daemon started."),
      ),
      el(
        "div",
        { class: "card pad" },
        el("span", { class: "card-title", text: "Model" }),
        el(
          "div",
          { class: "rows" },
          stores.map((s) =>
            el(
              "div",
              { class: "kv" },
              el("span", { class: "k", text: s.name }),
              el("span", { class: "v", text: `${s.model} · ${s.dim} dims` }),
            ),
          ),
        ),
        el("p", {
          class: "subtitle",
          text: "Fixed when the store was created. Vectors from two models are not comparable, so switching means indexing again.",
        }),
      ),
    ),
    ),
  );
}

// ----------------------------------------------------------------- files

async function filesView() {
  const chosenExt = new Set();
  const summary = el("span", { class: "pill" });
  const holder = el("div", { class: "grow" });

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

  async function forget(path, row) {
    try {
      await post("/api/forget", { path });
      // The row goes, and the page is reloaded behind it: with the server
      // paging, the row that moves up into the gap is on the server.
      row.remove();
      load();
    } catch (e) {
      fill(holder, error(e.message));
    }
  }

  const table = dataTable({
    className: "w-files",
    server: true,
    sort: "path",
    columns: [
      {
        key: "path",
        label: "Path",
        className: "path",
        // One line, with the whole path on the shared tooltip: a wrapped path
        // makes every row a different height and the column unreadable.
        render: (f) => el("span", { class: "one-line", "data-tip": f.path, text: f.path }),
      },
      { key: "store", label: "Store", className: "meta", sortable: false, render: (f) => f.store },
      {
        key: "reader",
        label: "Read as",
        sortable: false,
        render: (f) => el("span", { class: "tag", text: f.reader }),
      },
      { key: "lang", label: "Language", className: "meta", sortable: false, render: (f) => f.lang || "—" },
      { key: "lines", label: "Lines", className: "num", render: (f) => n(f.lines) },
      { key: "chunks", label: "Chunks", className: "num", render: (f) => n(f.chunks) },
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
              button.disabled = true;
              button.textContent = "Forgetting…";
              forget(f.path, button.closest("tr"));
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

    summary.textContent = `${n(data.total)} files · ${n(data.formats)} format${
      data.formats === 1 ? "" : "s"
    } · ${n(data.stores)} store${data.stores === 1 ? "" : "s"}`;

    if (!data.total) {
      fill(
        holder,
        el(
          "div",
          { class: "card pad" },
          empty("Nothing indexed matches that. The filter, not the corpus — clear it and look again."),
        ),
      );
      return;
    }
    table.update(data.files, data.total);
    if (!holder.contains(table.node)) fill(holder, table.node);
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

  return el(
    "div",
    { class: "view" },
    pageHead(
      "Files",
      "What is indexed, and which reader parsed it — so “not indexed” and “not discussed” stop looking the same.",
      { pill: summary },
    ),
    el(
      "div",
      { class: "filters" },
      el(
        "div",
        { class: "field wide" },
        icon(ICONS.search, 15),
        labelled("files-filter", "Filter files by path glob", pathInput),
      ),
      extChips,
    ),
    holder,
    el("p", {
      class: "subtitle",
      text: "Forget drops the file's chunks and vectors. The file on disk is untouched.",
    }),
  );
}

// ------------------------------------------------------------- languages

async function languagesView() {
  let data;
  try {
    data = await api("/api/languages");
  } catch (e) {
    return el("div", { class: "view" }, pageHead("Languages"), error(e.message));
  }
  const languages = data.languages || [];

  const tags = (list) => list.map((item) => el("span", { class: "tag wrapped", text: item }));

  const table = dataTable({
    className: "w-languages",
    sort: "name",
    perPage: 25,
    rows: languages,
    columns: [
      { key: "name", label: "Name", className: "path", value: (l) => l.name },
      {
        key: "extensions",
        label: "Extensions",
        value: (l) => (l.extensions || []).length,
        render: (l) => tags((l.extensions || []).map((e) => `.${e}`)),
      },
      {
        key: "filenames",
        label: "Filenames",
        value: (l) => (l.filenames || []).length,
        render: (l) =>
          (l.filenames || []).length ? tags(l.filenames) : el("span", { class: "meta", text: "—" }),
      },
    ],
  });

  return el(
    "div",
    { class: "view" },
    pageHead(
      "Languages",
      "Every name --lang accepts, on the command line, in the MCP tools and in the search box.",
      { pill: pill(`${languages.length} languages`) },
    ),
    languages.length ? table.node : empty("The language table is empty, which should be impossible."),
    el("p", {
      class: "subtitle",
      text: "Extension and filename decide the language; file contents are never read to guess it, because a store is searched far more often than it is built. The two names with no extension — dockerfile and makefile — match by filename instead.",
    }),
  );
}

// ---------------------------------------------------------------- search

/* Which of the three ranked lists found a hit. One letter each, with the name
 * on hover: a hit the graph alone reached is a neighbour of a match rather
 * than a match, and reading it as a match is the mistake this prevents. */
const LIST_LABELS = {
  vector: ["v", "vector — the embedding matched"],
  keyword: ["f", "full text — the terms matched"],
  graph: ["g", "graph — reached from a neighbouring symbol"],
};

function fusionBadges(lists) {
  if (!lists || !lists.length) return null;
  return el(
    "span",
    { class: "badges" },
    lists.map((list) => {
      const [letter, title] = LIST_LABELS[list] || [list[0], list];
      return el("span", { class: `badge ${list}`, title, text: letter });
    }),
  );
}

/** The likeliest symbol name in a hit, for the graph and impact links. */
function symbolIn(hit) {
  const match = hit.text.match(
    /(?:fn|function|def|func|class|struct|interface|type)\s+([A-Za-z_][A-Za-z0-9_]*)/,
  );
  return match ? match[1] : "";
}

async function searchView() {
  await refreshStores();

  const results = el("div", { class: "results" });
  const meta = el("span", { class: "search-meta" });
  const chosen = new Set();
  let generation = 0;

  const input = el("input", {
    type: "search",
    placeholder: "Ask it something",
    onkeydown: (e) => {
      if (e.key === "Enter") run();
    },
  });

  const nothing = () =>
    empty("Type a phrase or an identifier. Both halves of the search run either way.");

  async function run() {
    const query = input.value.trim();
    if (!query) {
      fill(results, nothing());
      meta.textContent = "";
      return;
    }

    const mine = ++generation;
    const params = new URLSearchParams({ query, k: "8" });
    for (const store of chosen) params.append("store", store);

    meta.textContent = "searching…";
    let data;
    try {
      data = await api(`/api/search?${params}`);
    } catch (e) {
      if (mine !== generation) return;
      fill(results, error(e.message));
      meta.textContent = "";
      return;
    }
    if (mine !== generation) return;

    meta.textContent = data.hits.length
      ? `${data.hits.length} of ${n(data.chunks)} chunks · ${Math.round(data.micros / 1000)} ms`
      : "";

    if (!data.hits.length) {
      fill(
        results,
        empty(
          "No chunk in the selected stores matches that. If a store chip is on it is the filter, not the corpus — clear it and ask again.",
        ),
      );
      return;
    }

    fill(
      results,
      data.hits.map((hit) =>
        el(
          "div",
          { class: "hit" },
          el(
            "div",
            { class: "where" },
            el("span", { class: "file", text: hit.path }),
            el("span", { class: "lines", text: `${hit.start_line}-${hit.end_line}` }),
            el("span", { class: "from", text: hit.store }),
            el("span", { class: "spacer" }),
            fusionBadges(hit.lists),
          ),
          el("pre", { text: hit.text }),
          el(
            "div",
            { class: "hit-actions" },
            el("a", {
              href: "#graph",
              class: "quiet",
              text: "Open in graph",
              onclick: (e) => {
                e.preventDefault();
                state.pendingSymbol = symbolIn(hit);
                go("graph");
              },
            }),
            el("a", {
              href: "#impact",
              class: "quiet",
              text: "Blast radius",
              onclick: (e) => {
                e.preventDefault();
                state.pendingSymbol = symbolIn(hit);
                go("impact");
              },
            }),
          ),
        ),
      ),
    );
  }

  const storeChips = state.stores.map((store) =>
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
        run();
      },
    }),
  );

  fill(results, nothing());
  setTimeout(() => {
    if (state.pendingQuery) {
      input.value = state.pendingQuery;
      state.pendingQuery = "";
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
        meta,
      ),
      el(
        "div",
        { class: "filters" },
        storeChips.length > 1 ? storeChips : null,
        el("span", { class: "spacer" }),
        el("span", { class: "meta", text: "vector + fts5 + graph, fused by rank" }),
      ),
    ),
    results,
  );
}

// ----------------------------------------------------------------- index

async function indexView() {
  await refreshStores();

  const log = el("div", { class: "log", "aria-live": "polite" });
  const bar = el("span", {});
  // Set through CSSOM rather than a style attribute: the CSP blocks the
  // attribute, and this is the one value that genuinely has to be dynamic.
  bar.style.width = "0%";
  const pct = el("span", { class: "pct", text: "0%" });
  const status = el("span", { class: "meta", text: "idle" });
  const picker = el("div", { class: "card picker", hidden: true });
  const note = el("div", { class: "note" });

  const field = el("input", {
    type: "text",
    placeholder: "~/work/api",
    value: state.pendingPath || "",
  });
  state.pendingPath = "";
  const urlField = el("input", { type: "text", placeholder: "link to a page, a PDF or a file" });

  const start = el("button", { class: "button", type: "button", text: "Start indexing" });
  const addButton = el("button", { class: "button secondary", type: "button", text: "Fetch and index" });

  const storeSelect = el(
    "select",
    { class: "chip static", "aria-label": "Store to write to" },
    state.stores.map((store) => el("option", { value: store.name, text: store.name })),
  );

  function say(text, key) {
    log.append(
      el(
        "div",
        { class: "line" },
        key ? el("span", { class: "key", text: key }) : null,
        el("span", { class: "what", text }),
      ),
    );
    log.scrollTop = log.scrollHeight;
  }

  function complain(message, focus) {
    note.className = "note bad";
    note.textContent = message;
    if (focus) focus.focus();
  }

  async function browse(path) {
    let data;
    try {
      data = await api(`/api/dirs?path=${encodeURIComponent(path || "")}`);
    } catch (e) {
      complain(e.message);
      return;
    }
    note.className = "note";
    note.textContent = "";
    picker.hidden = false;

    fill(
      picker,
      el(
        "div",
        { class: "crumbs" },
        el(
          "button",
          {
            class: "button secondary small",
            type: "button",
            disabled: !data.parent,
            onclick: () => browse(data.parent),
          },
          icon(ICONS.up, 15),
          "Up",
        ),
        el("span", { class: "where", text: data.path }),
        el("span", { class: "spacer" }),
        el("button", {
          class: "button small",
          type: "button",
          text: "Use this folder",
          onclick: () => {
            field.value = data.path;
            picker.hidden = true;
          },
        }),
        el("button", {
          class: "button ghost small",
          type: "button",
          text: "Close",
          onclick: () => {
            picker.hidden = true;
          },
        }),
      ),
      el(
        "div",
        { class: "entries" },
        data.entries.length
          ? data.entries.map((entry) =>
              el(
                "button",
                {
                  class: "entry",
                  type: "button",
                  onclick: () => {
                    if (entry.dir) browse(entry.path);
                    else {
                      field.value = entry.path;
                      picker.hidden = true;
                    }
                  },
                },
                icon(entry.dir ? ICONS.folder : ICONS.file),
                el("span", { class: "name", text: entry.name }),
              ),
            )
          : el("div", { class: "card pad" }, empty("Nothing here that semlith can index.")),
      ),
    );
  }

  /* One reader for both buttons. /api/add fetches the URL and then hands what
   * landed to the same write queue a folder goes through, so it answers with
   * the same event stream and there is no second progress mechanism. */
  async function run(route, body) {
    start.disabled = true;
    addButton.disabled = true;
    note.className = "note";
    note.textContent = "";
    log.replaceChildren();
    say(`POST ${route} · newline-delimited JSON, chunked`, "→");
    bar.style.width = "0%";
    pct.textContent = "0%";
    status.textContent = "working";

    try {
      const response = await fetch(route, {
        method: "POST",
        credentials: "same-origin",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ ...body, store: storeSelect.value || undefined }),
      });
      if (!response.ok) throw new Error((await response.text()) || response.statusText);

      // The response is chunked and arrives while the writer works, so it is
      // read as it comes rather than awaited whole — the point of streaming it.
      const reader = response.body.getReader();
      const decoder = new TextDecoder();
      let buffer = "";
      for (;;) {
        const { value, done } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        let cut;
        while ((cut = buffer.indexOf("\n")) >= 0) {
          const line = buffer.slice(0, cut).trim();
          buffer = buffer.slice(cut + 1);
          if (!line) continue;
          const event = JSON.parse(line);
          if (event.event === "file") {
            const share = event.total ? (event.scanned / event.total) * 100 : 0;
            const shown = Math.min(100, share);
            bar.style.width = `${shown.toFixed(1)}%`;
            pct.textContent = `${Math.round(shown)}%`;
            say(event.path, `${event.scanned}/${event.total}`);
          } else if (event.event === "done") {
            bar.style.width = "100%";
            pct.textContent = "100%";
            status.textContent = "done";
            say(
              `${event.indexed} indexed, ${event.unchanged} unchanged, ${event.skipped} skipped, ${event.removed} removed, ${event.chunks} chunks`,
              "done",
            );
            if (event.remaining) {
              say(
                `${event.remaining} paths left after the time slice — start again to continue; nothing already indexed is redone`,
                "note",
              );
            }
            // The store list has changed. Without this, a first index left
            // `state.stores` empty and the next visit to Stores bounced the
            // user to the first-run screen with their work apparently gone.
            await refreshStores();
          } else if (event.event === "error") {
            status.textContent = "failed";
            say(event.error, "error");
          } else if (event.event === "started") {
            say(event.paths.join(", "), "indexing");
          }
        }
      }
    } catch (e) {
      status.textContent = "failed";
      say(e.message, "error");
    } finally {
      start.disabled = false;
      addButton.disabled = false;
    }
  }

  start.addEventListener("click", () => {
    const path = field.value.trim();
    if (!path) {
      complain("Give a path to index, or choose a folder.", field);
      return;
    }
    run("/api/index", { path });
  });

  addButton.addEventListener("click", () => {
    const url = urlField.value.trim();
    if (!url) {
      complain("Give an https URL to fetch.", urlField);
      return;
    }
    run("/api/add", { url });
  });

  return el(
    "div",
    { class: "view" },
    pageHead(
      "Index",
      "The daemon is the writer, so this queues behind the watcher rather than fighting it — the same path semlith_index takes.",
    ),
    el(
      "div",
      { class: "filters" },
      el(
        "div",
        { class: "field tall grow" },
        el("span", { class: "prefix", text: "path" }),
        labelled("index-path", "Path to index", field),
      ),
      el(
        "button",
        { class: "button secondary", type: "button", onclick: () => browse("") },
        icon(ICONS.folder),
        "Choose folder…",
      ),
      state.stores.length > 1 ? storeSelect : null,
      start,
      el("span", { class: "spacer" }),
      el("span", { class: "meta", text: "writer: daemon" }),
    ),
    note,
    el(
      "div",
      { class: "scroller" },
    picker,
    el(
      "div",
      { class: "card pad" },
      el("span", { class: "eyebrow", text: "Add from a URL" }),
      el("p", {
        class: "subtitle",
        text: "One https request, for exactly this URL — a page, a PDF, or a file on GitHub. Nothing is crawled and no credential is ever sent. The file is saved inside this store's downloads folder, never in your working tree.",
      }),
      el(
        "div",
        { class: "filters" },
        el(
          "div",
          { class: "field tall grow" },
          el("span", { class: "prefix", text: "url" }),
          labelled("index-url", "URL to fetch and index", urlField),
        ),
        addButton,
      ),
    ),
    el(
      "div",
      { class: "card pad" },
      el(
        "div",
        { class: "head" },
        el("span", { class: "card-title", text: "Progress" }),
        status,
        el("span", { class: "spacer" }),
        pct,
      ),
      el("div", { class: "bar" }, bar),
    ),
    log,
    ),
  );
}

// -------------------------------------------------------- install panel

function installPanel() {
  const card = el("div", { class: "card pad install-panel" });
  const title = () => el("span", { class: "card-title", text: "Install and setup" });
  fill(card, title(), el("p", { class: "subtitle", text: "Checking this machine…" }));

  const stateWord = {
    done: "done",
    "already-done": "already done",
    skipped: "not done",
    failed: "failed",
  };

  api("/api/setup")
    .then((setup) => {
      const result = el("div", { class: "note" });

      const install = el("button", {
        class: "button small",
        type: "button",
        text: "Install it",
        disabled: true,
        onclick: async () => {
          install.disabled = true;
          check.disabled = true;
          result.className = "note";
          result.textContent = "Downloading and verifying…";
          try {
            const done = await post("/api/upgrade", { action: "apply" });
            result.textContent = done.restart;
          } catch (e) {
            result.className = "note bad";
            result.textContent = e.message;
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
          result.className = "note";
          result.textContent = "Asking GitHub…";
          try {
            const found = await post("/api/upgrade", { action: "check" });
            result.textContent = found.available
              ? `${found.latest} is available; you are on ${found.installed}.`
              : `Already on the newest release, ${found.installed}.`;
            if (found.blocked) result.textContent += ` ${found.blocked}`;
            install.disabled = !found.available || Boolean(found.blocked);
          } catch (e) {
            result.className = "note bad";
            result.textContent = e.message;
          } finally {
            check.disabled = false;
          }
        },
      });

      fill(
        card,
        title(),
        el("p", {
          class: "subtitle",
          text: "One command on a new machine. Nothing here reaches the network until you click it.",
        }),
        el("span", { class: "eyebrow", text: "macOS and Linux" }),
        copyField(setup.install_sh),
        el("span", { class: "eyebrow", text: "Windows" }),
        copyField(setup.install_ps1),
        el("hr", { class: "rule" }),
        el(
          "div",
          { class: "rows" },
          (setup.steps || []).map((step) =>
            el(
              "div",
              { class: "kv" },
              pill(
                stateWord[step.state] || step.state,
                step.state === "done" || step.state === "already-done" ? "good" : "warn",
              ),
              el("span", { class: "card-title", text: step.name }),
              el("span", { class: "meta", text: step.detail }),
            ),
          ),
        ),
        el("p", {
          class: "subtitle",
          text: setup.on_path
            ? `${setup.bin_dir} is on PATH.`
            : `${setup.bin_dir} is not on PATH — run \`semlith setup\` to add it.`,
        }),
        el("hr", { class: "rule" }),
        el("span", { class: "eyebrow", text: `Installed version ${setup.version}` }),
        el("div", { class: "actions" }, check, install),
        result,
      );
    })
    .catch((e) => fill(card, title(), error(e.message)));

  return card;
}

// ---------------------------------------------------------------- agents

async function agentsView() {
  let data;
  try {
    data = await api("/api/agents");
  } catch (e) {
    return el("div", { class: "view" }, pageHead("Agents"), error(e.message));
  }

  const clients = data.clients || [];
  const tools = data.tools || [];
  const revisions = data.revisions || [];
  const body = el("div", { class: "card pad" });

  function show(i) {
    const client = clients[i];
    if (!client) {
      fill(body, empty("No client stanza is compiled into this build."));
      return;
    }
    fill(
      body,
      el(
        "div",
        { class: "filters" },
        clients.map((c, j) =>
          el("button", {
            class: "chip",
            type: "button",
            "aria-pressed": String(j === i),
            text: c.name,
            onclick: () => show(j),
          }),
        ),
      ),
      el("span", { class: "card-title", text: client.name }),
      el("p", { class: "subtitle", text: client.note }),
      (client.stanzas || []).map((stanza) =>
        el(
          "div",
          { class: "rows" },
          el("span", { class: "eyebrow", text: stanza.format }),
          codeBlock(stanza.text.trimEnd(), "Copy stanza"),
        ),
      ),
    );
  }
  show(0);

  return el(
    "div",
    { class: "view" },
    pageHead("Agents", "One stanza, every store, no path to keep in step.", {
      pill: pill(
        data.forwarding
          ? `${data.connected} forwarding to this daemon`
          : "no client forwarding right now",
        data.forwarding ? "good" : "warn",
      ),
    }),
    el(
      "div",
      { class: "grid scroller" },
      el(
        "div",
        { class: "card pad" },
        el(
          "div",
          { class: "head" },
          el("span", { class: "card-title", text: "Tools exposed" }),
          el("span", { class: "meta", text: String(tools.length) }),
        ),
        tools.length
          ? el(
              "div",
              { class: "rows" },
              tools.map((tool) => el("span", { class: "path", text: tool })),
            )
          : empty("No tool is exposed, which should be impossible."),
        el("hr", { class: "rule" }),
        el("span", { class: "eyebrow", text: "Protocol revisions" }),
        el("span", { class: "meta", text: revisions.join(" · ") || "none advertised" }),
      ),
      body,
      installPanel(),
    ),
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

  const tokenBox = el("div", { class: "copyfield" });
  function showToken(text) {
    fill(tokenBox, el("code", { class: "text", text }));
  }
  showToken("held by this browser's cookie, not shown");

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
        showToken(fresh.token);
        rotateNote.textContent = "Rotated. The old token stopped working immediately.";
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
      fact("Telemetry", "none", "no analytics, and no update check semlith makes on its own"),
      fact("Model cache", data.model_cached ? "cached" : "not downloaded", data.model_cache),
    ),
    el(
      "div",
      { class: "grid scroller" },
      el(
        "div",
        { class: "card pad" },
        el("span", { class: "card-title", text: "Verify it with a packet capture" }),
        el("p", {
          class: "subtitle",
          text: "Watch every interface but loopback while you search. Nothing should appear.",
        }),
        copyField("sudo tcpdump -i any -n 'not host 127.0.0.1 and not host ::1'"),
        el("p", {
          class: "subtitle",
          text: "Or pull the cable: the portal loads and searches with no network at all.",
        }),
        el("hr", { class: "rule" }),
        el("span", { class: "card-title", text: "The outbound connections that exist" }),
        el("p", {
          class: "subtitle",
          text: "The embedding model is downloaded once, on first index, and cached. semlith upgrade and semlith add reach the network only in the second you ask them to. --airgap refuses all three and exits naming what it refused.",
        }),
        copyField("semlith index . --airgap"),
      ),
      el(
        "div",
        { class: "card pad" },
        el("span", { class: "card-title", text: "Session token" }),
        el("p", {
          class: "subtitle",
          text: `One token per run of the daemon, held in a SameSite=Strict ${data.token_cookie} cookie. Rotating it invalidates the old one immediately.`,
        }),
        tokenBox,
        el("div", { class: "actions" }, rotate),
        rotateNote,
        el("hr", { class: "rule" }),
        el("span", { class: "card-title", text: "Content-Security-Policy" }),
        codeBlock(data.csp),
        el("p", {
          class: "subtitle",
          text: `Host headers answered: ${(data.host_allowed || []).join(", ")}. Everything else gets 400.`,
        }),
      ),
    ),
  );
}

// ----------------------------------------------------------------- about

async function aboutView() {
  let about;
  let models;
  try {
    [about, models] = await Promise.all([api("/api/about"), api("/api/models")]);
  } catch (e) {
    return el("div", { class: "view" }, pageHead("About"), error(e.message));
  }

  const row = (key, value) =>
    el("div", { class: "kv" }, el("span", { class: "k", text: key }), el("span", { class: "v", text: value }));

  const list = (models && models.models) || [];

  return el(
    "div",
    { class: "view" },
    pageHead("About", "One Rust binary. The portal you are reading is compiled into it."),
    el(
      "div",
      { class: "grid scroller" },
      el(
        "div",
        { class: "card pad" },
        row("Version", about.version),
        row("Binary", about.binary),
        row("Port", String(about.port)),
        row("PID", String(about.pid)),
        row("Uptime", `${Math.floor(about.uptime / 60)}m`),
        row("Store home", about.store_home),
        row("Model cache", about.model_cache),
        row("Languages", String(about.languages)),
        row("Stores open", String(about.stores)),
      ),
      el(
        "div",
        { class: "card pad" },
        el("h2", { class: "card-title", text: "Languages with graph edges" }),
        el(
          "div",
          { class: "chips" },
          (about.graph_languages || []).map((lang) =>
            el("span", { class: "chip static", text: lang }),
          ),
        ),
        el("p", {
          class: "subtitle",
          text: `${(about.graph_languages || []).length} of ${about.languages} languages carry symbols and edges. The rest are searchable exactly as before, just without structure.`,
        }),
        el("h2", { class: "card-title", text: "Edge kinds" }),
        el(
          "div",
          { class: "chips" },
          (about.edge_kinds || []).map((kind) => el("span", { class: "chip static", text: kind })),
        ),
      ),
      el(
        "div",
        { class: "card" },
        list.length
          ? el(
              "div",
              { class: "table-wrap" },
              el(
                "table",
                { class: "w-models" },
                el(
                  "thead",
                  {},
                  el(
                    "tr",
                    {},
                    el("th", { text: "Model" }),
                    el("th", { class: "num", text: "Dims" }),
                    el("th", { text: "Note" }),
                  ),
                ),
                el(
                  "tbody",
                  {},
                  list.map((model) =>
                    el(
                      "tr",
                      {},
                      el("td", { class: "path", text: model.name }),
                      el("td", { class: "num", text: String(model.dim) }),
                      el("td", { class: "meta", text: model.description }),
                    ),
                  ),
                ),
              ),
            )
          : el("div", { class: "card pad" }, empty("No model is listed.")),
      ),
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
    el("div", { class: "lockup" }, logoImage(38), el("span", { class: "name", text: "semlith" })),
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
        el("button", {
          class: "button ghost",
          type: "button",
          text: "Skip for now",
          onclick: () => go("about"),
        }),
      ),
      el("hr", { class: "rule" }),
      el("span", { class: "eyebrow", text: "Or from a terminal" }),
      copyField("semlith index ~/Documents/work"),
      el("p", {
        class: "subtitle",
        text: "No --store flag. It lands in ~/.semlith/stores/work and is registered against that root.",
      }),
    ),
    el(
      "div",
      { class: "steps" },
      step("01", "Index", "One pass over the folder. Only what changed is re-embedded on the next."),
      step("02", "Stays current", "The daemon watches the roots and re-embeds on save."),
      step("03", "Ask", "From here, the CLI, or any agent over MCP — the same fused search."),
    ),
    el("div", { class: "foot", text: "127.0.0.1 · loopback only · no external asset" }),
  );
}

// ---------------------------------------------------------------- router

const RENDER = {
  stores: storesView,
  files: filesView,
  index: indexView,
  search: searchView,
  languages: languagesView,
  agents: agentsView,
  privacy: privacyView,
  about: aboutView,
  graph: graphView,
  impact: impactView,
  ledger: ledgerView,
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

function theme(next) {
  document.documentElement.setAttribute("data-theme", next);
  state.theme = next;
  try {
    localStorage.setItem("semlith-theme", next);
  } catch (_) {
    /* a private window refuses storage; the toggle still works for this run */
  }
  for (const img of document.querySelectorAll("img.logo")) {
    img.src = next === "dark" ? "logo-dark.svg" : "logo.svg";
  }
  paintThemeButton();
}

/** The toggle shows where it goes, not where you are. */
function paintThemeButton() {
  const button = shell.themeButton;
  if (!button) return;
  const toDark = !isDark();
  fill(button, icon(toDark ? ICONS.moon : ICONS.sun));
  button.setAttribute("aria-label", toDark ? "Switch to dark" : "Switch to light");
  button.setAttribute("title", toDark ? "Switch to dark" : "Switch to light");
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
    onclick: () => theme(isDark() ? "light" : "dark"),
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
        "aria-label": "semlith — go to Stores",
        onclick: () => go("stores"),
      },
      logoImage(26),
      el("span", { class: "wordmark", text: "semlith" }),
    ),
    el("span", { class: "divider-v" }),
    pageTitle,
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
      el("div", { class: "who" }, el("span", { class: "live-dot" }), "semlith start"),
      el("div", { class: "fact", text: "127.0.0.1 · sole writer" }),
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
  if (!state.stores.length && current !== "index" && current !== "about") {
    fill(root, welcomeView());
    shell.main = null;
    return;
  }

  if (!shell.main || !root.contains(shell.main)) {
    fill(root, buildShell());
    setNav(state.navOpen);
  }

  const view = VIEWS.find((v) => v.id === current) || VIEWS[0];
  shell.pageTitle.textContent = view.title;
  document.title = `semlith · ${view.title}`;
  markCurrent(view.id);

  paintStoreCount();

  const mine = ++renderGeneration;
  fill(shell.main, el("div", { class: "view" }, el("p", { class: "subtitle", text: "Loading…" })));
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
    state.theme = isDark() ? "dark" : "light";
  }

  wireTips();

  await refreshStores();
  await render();

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
