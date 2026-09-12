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
};

/* The nav marks, from the design. `|` separates subpaths so one mark can be
 * more than a single stroke. */
const NAV_ICONS = {
  stores: "M12 4l8 4-8 4-8-4 8-4|M4 12l8 4 8-4|M4 16.5l8 4 8-4",
  files: "M6 3h7l5 5v13H6z|M13 3v5h5",
  index: "M4 6h16|M4 12h10|M4 18h13",
  search: "M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14z|M16.2 16.2 20 20",
  languages: "M4 6h9|M8 6v2c0 3-2 5-4 6|M7 11c1 2 3 3 5 3.5|M13 20l4-9 4 9|M14.6 17h4.8",
  agents: "M9 3h6v5H9z|M12 8v3|M5 11h14v9H5z|M9 15h.01|M15 15h.01",
  privacy: "M12 3l7 3v6c0 4.3-3 7.3-7 9-4-1.7-7-4.7-7-9V6z",
  about: "M12 4a8 8 0 1 0 0 16 8 8 0 0 0 0-16z|M12 11v5|M12 8h.01",
};

/** A button that copies text and says so for a moment. */
function copyButton(getText, label) {
  const was = label || "Copy";
  const button = el("button", {
    class: "button secondary small",
    type: "button",
    text: was,
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
      button.textContent = "Copied";
      setTimeout(() => {
        button.textContent = was;
      }, 1400);
    },
  });
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

/** An input with a real label. A placeholder is not one: it leaves on typing. */
function labelled(id, text, input) {
  input.id = id;
  return [el("label", { class: "sr-only", for: id, text }), input];
}

function pageHead(title, subtitle) {
  return el(
    "div",
    { class: "titles" },
    el("h1", { text: title }),
    subtitle ? el("p", { class: "subtitle", text: subtitle }) : null,
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
};

/* Pages, in the design's grouping. The ones the roadmap puts in a later
 * release — Graph, Impact, Inside the index, Ledger, Reports, License — are
 * deliberately absent rather than stubbed: a nav item that leads nowhere is
 * worse than one that does not exist yet. */
const VIEWS = [
  { group: "Workspace", id: "stores", label: "Stores", title: "Stores" },
  { group: "Workspace", id: "files", label: "Files", title: "Files" },
  { group: "Workspace", id: "index", label: "Index", title: "Index" },
  { group: "Explore", id: "search", label: "Search", title: "Search" },
  { group: "Explore", id: "languages", label: "Languages", title: "Languages" },
  { group: "Operate", id: "agents", label: "Agents", title: "Agents" },
  { group: "Operate", id: "privacy", label: "Privacy", title: "Privacy" },
  { group: "About", id: "about", label: "About", title: "About" },
];

/** Read the store list into `state`, so every view agrees on how many exist. */
async function refreshStores() {
  try {
    const data = await api("/api/stores");
    state.stores = data.stores || [];
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
    el(
      "div",
      { class: "head" },
      el(
        "div",
        { class: "titles" },
        el("h1", { text: "Stores" }),
        el("p", { class: "subtitle", text: "Everything indexed on this machine. Nothing leaves it." }),
      ),
      el(
        "div",
        { class: "actions" },
        el(
          "button",
          { class: "button", type: "button", onclick: () => go("index") },
          icon(ICONS.plus),
          "Index a folder",
        ),
      ),
    ),
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
                  el(
                    "span",
                    { class: `pill ${s.watching ? "good" : "warn"}` },
                    el("span", { class: "dot" }),
                    s.watching ? when(s.last_write) : "not watching",
                  ),
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
  );
}

// ----------------------------------------------------------------- files

async function filesView() {
  const chosenExt = new Set();
  const table = el("div", { class: "card" });
  const count = el("span", { class: "meta" });

  const pathInput = el("input", {
    type: "text",
    placeholder: "path glob, e.g. src/**",
    oninput: () => {
      clearTimeout(load.timer);
      load.timer = setTimeout(load, 200);
    },
  });

  /** Which request is current, so a slow one cannot overwrite a fast one. */
  let generation = 0;

  async function load() {
    const mine = ++generation;
    const params = new URLSearchParams();
    if (pathInput.value.trim()) params.set("path", pathInput.value.trim());
    for (const ext of chosenExt) params.append("ext", ext);

    let data;
    try {
      data = await api(`/api/files?${params}`);
    } catch (e) {
      if (mine !== generation) return;
      fill(table, el("div", { class: "card pad" }, error(e.message)));
      count.textContent = "";
      return;
    }
    if (mine !== generation) return;

    count.textContent = `${n(data.total)} files`;
    if (!data.files.length) {
      fill(
        table,
        el(
          "div",
          { class: "card pad" },
          empty("Nothing indexed matches that. The filter, not the corpus — clear it and look again."),
        ),
      );
      return;
    }

    let shown = data.files.length;
    const rows = data.files.map((f) => {
      const forget = el("button", {
        class: "forget",
        type: "button",
        text: "Forget",
        onclick: async () => {
          forget.disabled = true;
          forget.textContent = "Forgetting…";
          try {
            await post("/api/forget", { path: f.path });
            // Drop the row rather than reloading the whole table: the answer is
            // already known, and a reload throws away the scroll position.
            row.remove();
            shown -= 1;
            count.textContent = `${n(Math.max(0, data.total - 1))} files`;
            if (shown === 0) load();
          } catch (e) {
            forget.disabled = false;
            forget.textContent = "Forget";
            fill(last, error(e.message));
          }
        },
      });

      const last = el("td", {}, forget);
      const row = el(
        "tr",
        {},
        el("td", { class: "path", "data-tip": f.path, text: f.path }),
        el("td", { class: "meta", text: f.store }),
        el("td", {}, el("span", { class: "tag", text: f.reader })),
        el("td", { class: "meta", text: f.lang || "—" }),
        el("td", { class: "num", text: n(f.lines) }),
        el("td", { class: "num", text: n(f.chunks) }),
        last,
      );
      return row;
    });

    fill(
      table,
      el(
        "div",
        { class: "table-wrap" },
        el(
          "table",
          { class: "w-files" },
          el(
            "thead",
            {},
            el(
              "tr",
              {},
              el("th", { text: "Path" }),
              el("th", { text: "Store" }),
              el("th", { text: "Read as" }),
              el("th", { text: "Language" }),
              el("th", { class: "num", text: "Lines" }),
              el("th", { class: "num", text: "Chunks" }),
              el("th", { text: "" }),
            ),
          ),
          el("tbody", {}, rows),
        ),
      ),
      data.files.length < data.total
        ? el(
            "div",
            { class: "table-foot" },
            el("span", {
              class: "meta",
              text: `showing the first ${n(data.files.length)} of ${n(data.total)} — narrow the filter to see the rest`,
            }),
          )
        : null,
    );
  }

  // The formats this release added are on the list, so they are one click away
  // rather than something you have to know to type.
  const EXTENSIONS = ["rs", "md", "py", "ts", "js", "go", "pdf", "docx", "epub", "rtf", "eml"];
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
        load();
      },
    }),
  );

  load();

  return el(
    "div",
    { class: "view" },
    el(
      "div",
      { class: "head" },
      el(
        "div",
        { class: "titles" },
        el("h1", { text: "Files" }),
        el("p", {
          class: "subtitle",
          text: "What is indexed, and which reader parsed it — so “not indexed” and “not discussed” stop looking the same.",
        }),
      ),
      count,
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
    table,
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

  const tags = (list) =>
    list.map((item) => el("span", { class: "tag wrapped", text: item }));

  return el(
    "div",
    { class: "view" },
    el(
      "div",
      { class: "head" },
      el(
        "div",
        { class: "titles" },
        el("h1", { text: "Languages" }),
        el("p", {
          class: "subtitle",
          text: "Every name --lang accepts, on the command line, in the MCP tools and in the search box.",
        }),
      ),
      el("span", { class: "pill", text: `${languages.length} languages` }),
    ),
    languages.length
      ? el(
          "div",
          { class: "card" },
          el(
            "div",
            { class: "table-wrap" },
            el(
              "table",
              { class: "w-languages" },
              el(
                "thead",
                {},
                el(
                  "tr",
                  {},
                  el("th", { text: "Name" }),
                  el("th", { text: "Extensions" }),
                  el("th", { text: "Filenames" }),
                ),
              ),
              el(
                "tbody",
                {},
                languages.map((language) =>
                  el(
                    "tr",
                    {},
                    el("td", { class: "path", text: language.name }),
                    el("td", {}, tags((language.extensions || []).map((e) => `.${e}`))),
                    (language.filenames || []).length
                      ? el("td", {}, tags(language.filenames))
                      : el("td", { class: "meta", text: "—" }),
                  ),
                ),
              ),
            ),
          ),
        )
      : empty("The language table is empty, which should be impossible."),
    el("p", {
      class: "subtitle",
      text: "Extension and filename decide the language; file contents are never read to guess it, because a store is searched far more often than it is built. The two names with no extension — dockerfile and makefile — match by filename instead.",
    }),
  );
}

// ---------------------------------------------------------------- search

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
          ),
          el("pre", { text: hit.text }),
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
  setTimeout(() => input.focus(), 0);

  return el(
    "div",
    { class: "search-page" },
    el(
      "div",
      { class: "search-band" },
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
        el("span", { class: "meta", text: "vector + fts5, fused by rank" }),
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
    el(
      "div",
      { class: "titles" },
      el("h1", { text: "Index" }),
      el("p", {
        class: "subtitle",
        text: "The daemon is the writer, so this queues behind the watcher rather than fighting it — the same path semlith_index takes.",
      }),
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
              el("span", {
                class: `pill ${step.state === "done" || step.state === "already-done" ? "good" : ""}`,
                text: stateWord[step.state] || step.state,
              }),
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
          el("pre", { class: "code", text: stanza.text.trimEnd() }),
          el("div", { class: "actions" }, copyButton(stanza.text.trimEnd(), "Copy stanza")),
        ),
      ),
    );
  }
  show(0);

  return el(
    "div",
    { class: "view" },
    el(
      "div",
      { class: "head" },
      el(
        "div",
        { class: "titles" },
        el("h1", { text: "Agents" }),
        el("p", { class: "subtitle", text: "One stanza, every store, no path to keep in step." }),
      ),
      el(
        "span",
        { class: `pill ${data.forwarding ? "good" : ""}` },
        el("span", { class: "dot" }),
        data.forwarding
          ? `${data.connected} forwarding to this daemon`
          : "no client forwarding right now",
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
    el(
      "div",
      { class: "head" },
      el(
        "div",
        { class: "titles" },
        el(
          "div",
          { class: "actions" },
          el("h1", { text: "Privacy" }),
          el("span", {
            class: `pill ${data.airgap ? "good" : ""}`,
            text: data.airgap ? "airgap mode armed" : "airgap off",
          }),
        ),
        el("p", {
          class: "subtitle",
          text: "The claim is “nothing leaves this machine”. This page is how you check it yourself, in about a minute.",
        }),
      ),
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
      { class: "grid" },
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
        el("pre", { class: "code", text: data.csp }),
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
    el(
      "div",
      { class: "titles" },
      el("h1", { text: "About" }),
      el("p", {
        class: "subtitle",
        text: "One Rust binary. The portal you are reading is compiled into it.",
      }),
    ),
    el(
      "div",
      { class: "grid" },
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
        el("h2", { text: "No stores yet" }),
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
};

function go(id) {
  location.hash = `#${id}`;
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
      onclick: () => setNav(!state.navOpen),
    },
    icon(ICONS.menu, 17),
  );

  const pageTitle = el("span", { class: "page-title" });

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
    el(
      "button",
      {
        class: "icon-button",
        type: "button",
        "aria-label": "Light or dark",
        onclick: () => theme(isDark() ? "light" : "dark"),
      },
      icon(ICONS.sun),
    ),
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
    onclick: () => setNav(false),
  });

  const main = el("main", {});
  const body = el("div", { class: "body" }, rail, sidebar, scrim, main);

  Object.assign(shell, { topbar, pageTitle, rail, sidebar, scrim, main, hamburger });
  return el("div", { id: "app" }, topbar, body);
}

/** Open or close the navigation without re-rendering the view inside it. */
function setNav(open) {
  state.navOpen = open;
  const narrow = window.matchMedia(NARROW).matches;
  shell.hamburger.setAttribute("aria-expanded", String(open));
  shell.sidebar.hidden = !open;
  // The rail is the wide-screen stand-in for the closed drawer. On a narrow
  // screen CSS hides it outright, so it is only ever a wide-screen concern.
  shell.rail.hidden = open || narrow;
  shell.scrim.hidden = !(open && narrow);
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

  const daemon = document.getElementById("daemon-stores");
  if (daemon) {
    daemon.textContent = `${state.stores.length} store${state.stores.length === 1 ? "" : "s"}`;
  }

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
      setNav(false);
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
