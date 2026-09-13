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
  alert: "M12 9v4|M12 17h.01|M12 4 3 19h18z",
  moon: "M20 14.5A8.5 8.5 0 0 1 9.5 4a8.5 8.5 0 1 0 10.5 10.5z",
  copy: "M9 9h9a1 1 0 0 1 1 1v9a1 1 0 0 1-1 1H9a1 1 0 0 1-1-1v-9a1 1 0 0 1 1-1z|M6 15H5a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1h9a1 1 0 0 1 1 1v1",
  // Three filled dots, as circles rather than as dotted strokes.
  more:
    "M12 4.1a2 2 0 1 0 0 4 2 2 0 0 0 0-4z|M12 10a2 2 0 1 0 0 4 2 2 0 0 0 0-4z|"
    + "M12 15.9a2 2 0 1 0 0 4 2 2 0 0 0 0-4z",
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
const PER_PAGE = [5, 10, 25, 50];

function dataTable(spec) {
  const columns = spec.columns;
  const view = {
    page: 1,
    perPage: spec.perPage || 10,
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
      el("span", { class: "meta", text: `Page ${n(view.page)} / ${n(pages())}` }),
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

/* The server-side folder picker.
 *
 * One component, used by the Index page to choose what to index and by the
 * Stores page to choose a directory to adopt. It browses the daemon's
 * filesystem rather than the browser's, because the daemon is what has to open
 * the path — a `<input type=file webkitdirectory>` hands over a list of file
 * objects and not the one thing needed, which is the path itself.
 */
function folderPicker(options) {
  const { onChoose, choose, onError } = options || {};
  const card = el("div", { class: "card picker", hidden: true });

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
        el("button", {
          class: "button small",
          type: "button",
          text: choose || "Use this folder",
          onclick: () => {
            card.hidden = true;
            if (onChoose) onChoose(data.path);
          },
        }),
        el("button", {
          class: "button ghost small",
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
          ? data.entries.map((entry) =>
              el(
                "button",
                {
                  class: "entry",
                  type: "button",
                  onclick: () => {
                    if (entry.dir) open(entry.path);
                    else {
                      card.hidden = true;
                      if (onChoose) onChoose(entry.path);
                    }
                  },
                },
                icon(entry.dir ? ICONS.folder : ICONS.file),
                el("span", { class: "name", text: entry.name }),
              ),
            )
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
    { class: className ? `one-line tail ${className}` : "one-line tail", "data-tip": value },
    el("bdi", { text: value }),
  );
}

/** A value on one line, truncated at the end, whole on hover. */
function lineCell(value, className) {
  return el("span", {
    class: className ? `one-line ${className}` : "one-line",
    "data-tip": value,
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
  /** Carried from a search hit into the Graph page, or into Search. */
  pendingSymbol: "",
  pendingQuery: "",
  /** The last search, kept so leaving the page and coming back does not throw
   * the question away along with its answers. */
  search: { query: "", stores: [] },
  /** How many clients are talking to the daemon, for the sidebar card. */
  agents: null,
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
  const parts = [`${many} store${many === 1 ? "" : "s"}`];
  // Only once the answer is known: "0 agents" while the route is still in
  // flight reads as a fact rather than as a question nobody has asked yet.
  if (state.agents !== null) {
    parts.push(`${state.agents} agent${state.agents === 1 ? "" : "s"}`);
  }
  node.textContent = parts.join(" · ");
}

/** How many clients are connected, from whichever page last asked. */
function noteAgents(data) {
  state.agents = (data.connections || []).length;
  paintStoreCount();
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
    sel: read("--accent", "#f0a43c"),
    selLine: read("--accent-edge", "#c97f14"),
    selText: read("--accent-ink", "#1e2a35"),
    muted: read("--muted", "#64778a"),
  };
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
  let drawn = [];
  let selected = null;
  let near = new Set();
  let hovered = null;
  let dragging = null;
  let down = null;
  let frame = null;
  let running = true;
  let settled = false;
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
      // can click.
      node.x = Math.min(0.94, Math.max(0.06, node.x + node.vx));
      node.y = Math.min(0.92, Math.max(0.08, node.y + node.vy));
    }
    alpha = Math.max(ALPHA_FLOOR, alpha * 0.985);
    return moved;
  }

  /** Put the heat back in, for anything that changes the layout. */
  function reheat(to) {
    alpha = Math.max(alpha, to === undefined ? 0.35 : to);
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
      // An inferred edge is dashed as well as coloured: colour alone is not a
      // distinction everyone can see.
      ctx.setLineDash(edge.confidence === "extracted" ? [] : [4, 4]);
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
      dragging.x = Math.min(0.94, Math.max(0.06, point.x / point.w));
      dragging.y = Math.min(0.92, Math.max(0.08, point.y / point.h));
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
      load = nodes.map(() => 1);
      for (const edge of edges) {
        if (nodes[edge.from]) nodes[edge.from].callees += 1;
        if (nodes[edge.to]) nodes[edge.to].callers += 1;
        if (load[edge.from] !== undefined) load[edge.from] += 1;
        if (load[edge.to] !== undefined) load[edge.to] += 1;
      }
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
      paint();
    },
    /** Draw only the edge kinds asked for. Returns how many are drawn. */
    filter(kinds) {
      drawn = !kinds || !kinds.size ? edges : edges.filter((e) => kinds.has(e.kind));
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
    counts: () => ({ nodes: nodes.length, edges: drawn.length }),
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

const EDGE_KINDS = ["calls", "imports", "defines", "references"];

async function graphView() {
  await refreshStores();

  const kinds = new Set(EDGE_KINDS);
  const chosen = new Set();
  const meta = el("span", { class: "graph-count" });
  const rail = el("div", { class: "graph-rail" });

  const canvas = graphCanvas({
    onPick: (node) => select(node),
    onHover: (node, x, y) => {
      if (!node) return tip.hide("graph");
      tip.atPoint(
        x,
        y,
        [
          el(
            "div",
            { class: "tip-head" },
            el("i", {}),
            node.name,
          ),
          row("kind", node.kind),
          row("file", `${shortPath(node.path)}:${node.start_line}-${node.end_line}`),
          row("store", node.store || (state.stores[0] && state.stores[0].name) || "—"),
          row("calls", `${node.callers} in · ${node.callees} out`),
        ],
        "graph",
      );
    },
  });

  function row(key, value) {
    return el(
      "div",
      { class: "tip-row" },
      el("span", { class: "k", text: key }),
      el("span", { class: "v", text: String(value) }),
    );
  }

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

  function counts() {
    const { nodes, edges } = canvas.counts();
    meta.textContent = `${n(nodes)} symbols · ${n(edges)} edges`;
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
    const edge = (e) => ({ name: e.name, kind: e.kind, confidence: e.confidence });

    fill(
      rail,
      el(
        "div",
        { class: "graph-selected" },
        el("span", { class: "eyebrow", text: "Selected symbol" }),
        el("h2", { class: "sym", text: node.name }),
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
      ),
      ends("Callers", data.callers.map(edge), "Nothing in the graph calls this."),
      ends("Callees", data.callees.map(edge), "A leaf, as far as the extracted edges go."),
      el(
        "div",
        { class: "rail-actions" },
        el("a", {
          class: "button secondary small",
          href: "#search",
          text: "Chunks it lives in",
          onclick: (e) => {
            e.preventDefault();
            state.pendingQuery = node.name;
            go("search");
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
        counts();
      },
    }),
  );

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
    { class: "graph-page" },
    el(
      "div",
      { class: "graph-band" },
      pageHead(
        "Graph",
        "Edges are re-extracted on the same pass that re-embeds a file. Never a stale build artifact.",
        { actions: [el("div", { class: "filters" }, kindChips), pause] },
      ),
      el(
        "div",
        { class: "graph-controls" },
        el(
          "div",
          { class: "graph-scope" },
          icon(ICONS.search, 16),
          labelled("graph-scope", "Scope the graph", scopeInput),
        ),
        storeChips.length > 1 ? el("div", { class: "filters" }, storeChips) : null,
      ),
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
          el("span", { class: "key inferred" }, el("i", {}), "inferred"),
          el("span", { class: "key selected" }, el("i", {}), "selected"),
        ),
        meta,
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
  return el(
    "div",
    { class: "view" },
    pageHead(
      "Retrieval ledger",
      "Every query an agent ran, recorded locally. The honest token number, a debugging trail, and an audit record that never left the machine.",
      { pill: pill(on ? "recording · opt-in" : "not recording", on ? "on" : null) },
    ),
    el(
      "div",
      { class: "strip" },
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
      el(
        "div",
        { class: "card pad dense" },
        copyField("semlith ledger --last 20"),
        el("p", { class: "subtitle", text: "Prints the ledger on the command line." }),
      ),
      on
        ? null
        : el(
            "div",
            { class: "notice" },
            el("div", { class: "what", text: "Recording is off" }),
            el(
              "div",
              {},
              "Start the daemon with ",
              mono("--ledger"),
              " to record what your agents retrieve. Nothing is sent anywhere; the rows live in the store beside the chunks.",
            ),
          ),
      says(
        "Stored in ",
        mono("~/.semlith/stores/<name>/store.db"),
        ", table ",
        mono("retrievals"),
        ". Off by default; deleting the rows is a ",
        mono("DELETE"),
        ".",
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
    el("button", {
      class: "button ghost small",
      type: "button",
      text: "Copy config for another client",
      onclick: () => go("agents"),
    }),
  );
  fill(body, el("div", { class: "rail-hint", text: "Asking the daemon…" }));

  api("/api/agents")
    .then((data) => {
      noteAgents(data);
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

/** Ask before deleting a store, in the page rather than in a browser dialog.
 *
 * A store is minutes of embedding, so the confirmation says what goes and what
 * does not, and the button that does it is the red one. */
function confirmDelete(name) {
  const holder = document.querySelector(".view > .note.page-note");
  if (!holder) return;
  holder.className = "note page-note asking";
  const go = el("button", {
    class: "button danger small",
    type: "button",
    text: `Delete ${name}`,
    onclick: async () => {
      go.disabled = true;
      go.textContent = "Deleting…";
      try {
        const done = await post("/api/store/delete", { store: name });
        note(done.message);
        await refreshStores();
        render();
      } catch (e) {
        note(e.message, true);
      }
    },
  });
  fill(
    holder,
    el("span", {
      text: `Delete ${name}? Its vectors, chunks, graph and ledger go. The files it indexed are untouched.`,
    }),
    el("span", { class: "spacer" }),
    el("button", {
      class: "button ghost small",
      type: "button",
      text: "Keep it",
      onclick: () => {
        holder.className = "note page-note";
        holder.textContent = "";
      },
    }),
    go,
  );
}

/** A line under the page head, for something that just happened to the page. */
function note(text, bad) {
  const holder = document.querySelector(".view > .note.page-note");
  if (!holder) return;
  holder.className = bad ? "note page-note bad" : "note page-note";
  holder.textContent = text;
}

async function storesView() {
  const stores = await refreshStores();

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

  const table = dataTable({
    className: "w-stores",
    sort: "name",
    perPage: 10,
    rows: stores,
    columns: [
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
            // The model per store, which the About page states only once for
            // the machine. Two stores can have been built with two models and
            // their vectors are not comparable, so it belongs beside the row.
            lineCell(`${s.model} · ${s.dim} dims`, "meta"),
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
        key: "last_write",
        label: "Last write",
        value: (s) => s.last_write || 0,
        // Green means a write landed. A store that is watched and has never
        // been written to is neither good news nor a warning, so it carries a
        // plain pill with no dot at all.
        render: (s) => {
          if (!s.watching) return pill("not watching", "warn");
          return s.last_write ? pill(when(s.last_write), "good") : pill("never");
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
            { label: "Open in Files", onclick: () => go("files") },
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

  return el(
    "div",
    { class: "view" },
    head,
    picker.node,
    adoptNote,
    rootPicker.node,
    rootNote,
    el("div", { class: "note page-note" }),
    el(
      "div",
      { class: "strip" },
      stat("Stores", n(stores.length), `${totals.watching} being watched`),
      stat("Files", n(totals.files), `${n(totals.formats)} formats`),
      stat("Chunks", n(totals.chunks), dim ? `${dim}-dimension vectors` : "embedded and searchable"),
      stat("Lines", n(totals.lines), `${n(totals.readers)} readers in use`),
      stat("On disk", bytes(totals.bytes), "int8 quantised"),
    ),
    el(
      "div",
      { class: "scroller" },
      table.node,
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
        agentsCard(),
      ),
    ),
  );
}

// ----------------------------------------------------------------- files

async function filesView() {
  const chosenExt = new Set();
  const summary = el("span", { class: "pill" });
  const holder = el("div", {});

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
      load();
    } catch (e) {
      // In the note, not over the table: an error that replaces the rows
      // takes away the thing the reader was working with.
      bulkNote.className = "note bad";
      bulkNote.textContent = e.message;
    }
  }

  /* The rows ticked for a bulk forget, by path.
   *
   * Kept out here rather than in the table, because the table re-renders on
   * every sort, page and filter and a selection that vanished when you sorted
   * would be a selection nobody could trust. */
  const picked = new Map();
  const bulkNote = el("div", { class: "note" });
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
      el("span", { class: "spacer" }),
      el("button", {
        class: "button ghost small",
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
        onclick: (e) => forgetPicked(e.currentTarget),
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
    }
  }

  const table = dataTable({
    className: "w-files",
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
        // One line, with the whole path on the shared tooltip: a wrapped path
        // makes every row a different height and the column unreadable.
        render: (f) => pathCell(f.path),
      },
      {
        key: "store",
        label: "Store",
        className: "meta narrow-drop",
        sortable: false,
        render: (f) => f.store,
      },
      {
        key: "reader",
        label: "Read as",
        className: "narrow-drop",
        sortable: false,
        render: (f) => el("span", { class: "tag", text: f.reader }),
      },
      {
        key: "lang",
        label: "Language",
        className: "meta narrow-drop",
        sortable: false,
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
              button.disabled = true;
              button.textContent = "Forgetting…";
              forget(f.path, button.closest("tr"), f.store);
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
    bulkBar,
    bulkNote,
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
        className: "narrow-drop",
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
  image: ["i", "image — the picture matched the words"],
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

/** The likeliest symbol name in a hit, for the link into the graph. */
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
  // Restored rather than reset: the question and the store filter survive a
  // trip to another page, because coming back to an empty box means typing it
  // again.
  const chosen = new Set(state.search.stores);
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
    state.search = { query, stores: [...chosen] };
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
            el("span", {
              class: "lines",
              text: hit.image
                ? `${hit.image.width}×${hit.image.height} px`
                : `${hit.start_line}-${hit.end_line}`,
            }),
            el("span", { class: "from", text: hit.store }),
            el("span", { class: "spacer" }),
            fusionBadges(hit.lists),
          ),
          // An image has no excerpt to quote, so the hit shows the image. It
          // is served from the store's own list of indexed images, so the
          // route cannot be asked for a file nobody pointed semlith at.
          hit.image
            ? el("img", {
                class: "preview",
                src: `/api/image?path=${encodeURIComponent(hit.path)}`,
                alt: "",
              })
            : el("pre", { text: hit.text }),
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
  for (const chip of storeChips) {
    chip.setAttribute("aria-pressed", String(chosen.has(chip.textContent)));
  }

  fill(results, nothing());
  setTimeout(() => {
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

  // Hidden until there is something to say. An empty terminal-coloured slab
  // under an idle page is a block of nothing pretending to be output.
  const log = el("div", { class: "log", "aria-live": "polite", hidden: true });
  const bar = el("span", {});
  // Set through CSSOM rather than a style attribute: the CSP blocks the
  // attribute, and this is the one value that genuinely has to be dynamic.
  bar.style.width = "0%";
  const pct = el("span", { class: "pct", text: "0%" });
  const status = el("span", { class: "meta", text: "idle" });
  const picker = folderPicker({
    onChoose: (path) => {
      field.value = path;
    },
    onError: (message) => complain(message),
  });
  const note = el("div", { class: "note" });
  const urlCard = el("div", { class: "card pad", hidden: true });
  const urlNote = el("div", { class: "note" });

  const folderButton = el(
    "button",
    {
      class: "button secondary",
      type: "button",
      "aria-pressed": "false",
      onclick: () => reveal("picker"),
    },
    icon(ICONS.folder),
    "Choose folder…",
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

  /* The two ways to start a queue are mutually exclusive: showing a folder
   * picker and a URL field at once offers two answers to one question. Both
   * start closed, so the page opens on the path field and its actions, and the
   * button that opened one stays lit while it is open. */
  function reveal(which) {
    const wantPicker = which === "picker" && !picker.isOpen();
    const wantUrl = which === "url" && urlCard.hidden;
    if (!wantPicker) picker.close();
    urlCard.hidden = !wantUrl;
    if (wantPicker) picker.open("");
    if (wantUrl) urlField.focus();
    folderButton.setAttribute("aria-pressed", String(wantPicker));
    urlButton.setAttribute("aria-pressed", String(wantUrl));
  }

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
    { class: "field", "aria-label": "Store to write to" },
    state.stores.map((store) => el("option", { value: store.name, text: store.name })),
  );

  function say(text, key) {
    log.hidden = false;
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
      if (!response.ok) {
        // The body is the same `{"error": ...}` shape every other route
        // answers with, and showing the JSON rather than the sentence inside
        // it is how a clear message arrives looking like a fault.
        const body = await response.text();
        let detail = body;
        try {
          detail = JSON.parse(body).error || body;
        } catch (_) {
          /* a plain-text body is fine too */
        }
        throw new Error(detail || response.statusText);
      }

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
      return e.message;
    } finally {
      start.disabled = false;
      addButton.disabled = false;
    }
    return null;
  }

  start.addEventListener("click", () => {
    const path = field.value.trim();
    if (!path) {
      complain("Give a path to index, or choose a folder.", field);
      return;
    }
    run("/api/index", { path });
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
    const failed = await run("/api/add", { url });
    // A failed fetch leaves the card open with what went wrong on it. Closing
    // it would take the message away with it and leave the page looking as
    // though nothing had been asked for.
    if (failed) {
      urlNote.className = "note bad";
      urlNote.textContent = failed;
      urlCard.hidden = false;
    }
  });

  return el(
    "div",
    { class: "view" },
    pageHead("Index"),
    says(
      "The daemon is the writer, so this queues behind the watcher rather than fighting it — the same path ",
      mono("semlith_index"),
      " takes.",
    ),
    // The path field has the row to itself, above the actions: sharing a row
    // with four buttons is what held it to 420px on a 1440px page.
    el(
      "div",
      { class: "field tall full" },
      el("span", { class: "prefix", text: "path" }),
      labelled("index-path", "Path to index", field),
    ),
    el(
      "div",
      { class: "filters" },
      folderButton,
      urlButton,
      state.stores.length > 1 ? storeSelect : null,
      start,
      el("span", { class: "spacer" }),
      // What the queue actually holds, from /api/stores. The line it replaces
      // said "writer: daemon", which named a process rather than telling
      // anyone anything about their work.
      el("span", {
        class: "meta",
        text: `queue depth ${state.stores.reduce((sum, store) => sum + (store.queue || 0), 0)}`,
      }),
    ),
    note,
    el(
      "div",
      { class: "scroller" },
    picker.node,
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
      el("div", { class: "actions" }, addButton),
      urlNote,
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
    onclick: async () => {
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
    },
  });
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
        stanzas.key = done.key;
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
          ? `New key. ${rewrote} The previous one keeps working until this daemon exits, so a session already open finishes — any client configured elsewhere needs the new stanza before then.`
          : `New key. ${rewrote} The previous one is refused now; any client configured elsewhere needs the new stanza.`;
      } catch (e) {
        keyNote.className = "note bad";
        keyNote.textContent = e.message;
      } finally {
        rotate.disabled = false;
      }
    },
  });

  // ---- connected clients
  const connected = dataTable({
    className: "w-agents",
    sort: "name",
    grow: false,
    perPage: 10,
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
  const stanzas = { key: data.key || "" };
  let group = GROUPS.find(([id]) => clients.some((c) => c.group === id))[0];
  let chosen = clients.findIndex((c) => c.group === group);
  const tabs = el("div", { class: "tabs" });
  const chips = el("div", { class: "filters" });
  const body = el("div", { class: "stanza" });

  /** The HTTP form, built here from the key the route just handed back. */
  /** The placeholder the README prints where a real key goes. */
  const KEY_SLOT = "sml_YOURKEY";

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
    // The documented stanzas, with this daemon's key in them: a reader who
    // copies one should not have to find `sml_YOURKEY` and paste the key over
    // it, and the page already knows the key.
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
    keyNote,
    el(
      "div",
      { class: "grid scroller" },
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
            el("span", { class: "meta", text: String(tools.length) }),
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
        installPanel(),
      ),
      el("div", { class: "card pad stanzas" }, tabs, chips, body),
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
        // The rotate response is the one place the whole token appears, and it
        // sets the new cookie in the same breath. What is shown is still the
        // preview: a page that prints a live credential in full is a page
        // someone screenshots.
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
      fact("Model cache", data.model_cached ? "cached" : "not downloaded", data.model_cache),
    ),
    el(
      "div",
      { class: "grid scroller" },
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
        ),
      ),
      el(
        "div",
        { class: "rows" },
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
        el(
          "div",
          { class: "card pad" },
          el("span", { class: "card-title", text: "Session token" }),
          el("div", { class: "copyfield" }, tokenBox, el("div", { class: "actions" }, rotate)),
          rotateNote,
          says(
            "Generated at start, held in a SameSite=Strict ",
            mono(data.token_cookie),
            " cookie, and required on every ",
            mono("/api"),
            " route. Shown truncated: no response carries it in full except the one that rotates it, which sets the new cookie in the same breath.",
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
    el(
      "div",
      { class: "kv pair" },
      el("span", { class: "k", text: key }),
      el("span", { class: "v", text: value }),
    );

  const list = (models && models.models) || [];
  const table = dataTable({
    className: "w-models",
    sort: "name",
    perPage: 10,
    rows: list,
    columns: [
      {
        key: "name",
        label: "Model",
        className: "path",
        value: (m) => m.name,
        // One line with the name on the tooltip: wrapped across two lines a
        // model name reads as two models.
        render: (m) => el("span", { class: "one-line", "data-tip": m.name, text: m.name }),
      },
      { key: "dim", label: "Dims", className: "num", value: (m) => m.dim, render: (m) => String(m.dim) },
      {
        key: "bytes",
        label: "Size",
        className: "num",
        value: (m) => m.bytes || 0,
        // Blank rather than guessed: a model this machine has never fetched has
        // no size here to measure, and fastembed's catalogue does not carry one.
        render: (m) => (m.bytes ? bytes(m.bytes) : "—"),
      },
      { key: "description", label: "Note", className: "meta", value: (m) => m.description },
    ],
  });

  return el(
    "div",
    { class: "view" },
    pageHead("About", "One Rust binary. The portal you are reading is compiled into it."),
    el(
      "div",
      { class: "grid two grow" },
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
          row("MCP revisions", (about.revisions || []).join(" · ")),
          row("Uptime", `${Math.floor(about.uptime / 60)}m · pid ${about.pid}`),
        ),
        el(
          "div",
          { class: "card pad" },
          el("span", { class: "card-title", text: "Languages with graph edges" }),
          el(
            "div",
            { class: "chips" },
            (about.graph_languages || []).map((lang) =>
              el("span", { class: "chip static blue", text: lang }),
            ),
          ),
          says(
            "Everything else is indexed and searchable; it just has no edges yet. ",
            mono("semlith languages"),
            ` lists all ${about.languages} that `,
            mono("--lang"),
            " accepts.",
          ),
        ),
      ),
      list.length ? table.node : el("div", { class: "card pad" }, empty("No model is listed.")),
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
    el("div", { class: "lockup" }, logoImage(38), el("span", { class: "name", text: "Semlith" })),
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
      says(
        "No ",
        mono("--store"),
        " flag. It lands in ",
        mono("~/.semlith/stores/work"),
        " and is registered against that root.",
      ),
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
        "aria-label": "Semlith — go to Stores",
        onclick: () => go("stores"),
      },
      logoImage(26),
      el("span", { class: "wordmark", text: "Semlith" }),
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
  document.title = `Semlith · ${view.title}`;
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
  // Not awaited: the sidebar's agent count is a detail, and `claude mcp list`
  // behind this route is slow on some machines. It fills itself in.
  api("/api/agents").then(noteAgents).catch(() => {});
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
