/* semlith portal
 *
 * Plain DOM, no framework, no build step. The whole file is served from the
 * binary, so there is nothing to bundle and nothing to fetch: what you read
 * here is what runs.
 *
 * Every view is a function that returns an element. The router swaps one for
 * another. That is the entire architecture, and it is enough for eight views.
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

/** A button that copies text and says so for a moment, as the design does. */
function copyButton(getText, label) {
  const button = el("button", {
    class: "button secondary small",
    type: "button",
    onclick: async () => {
      const text = typeof getText === "function" ? getText() : getText;
      try {
        await navigator.clipboard.writeText(text);
      } catch (_) {
        // No clipboard permission, or an insecure context. Selecting the text
        // is still a way to copy it, and silently doing nothing is not.
        const area = el("textarea", { style: "position:fixed;opacity:0" });
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
      const was = button.textContent;
      button.textContent = "Copied";
      setTimeout(() => {
        button.textContent = was;
      }, 1400);
    },
    text: label || "Copy",
  });
  return button;
}

function copyField(command) {
  return el("div", { class: "copyfield" }, el("code", { text: command }), copyButton(command));
}

function icon(d) {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("width", "16");
  svg.setAttribute("height", "16");
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "1.8");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
  path.setAttribute("d", d);
  svg.append(path);
  return svg;
}

const ICONS = {
  menu: "M4 7h16M4 12h16M4 17h16",
  search: "M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14zM20 20l-4-4",
  plus: "M12 5v14M5 12h14",
  sun: "M12 4v1M12 19v1M4 12h1M19 12h1M6.3 6.3l.7.7M17 17l.7.7M17.7 6.3l-.7.7M7 17l-.7.7M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8z",
  folder: "M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z",
  file: "M6 3h7l5 5v13H6zM13 3v5h5",
};

function error(message) {
  return el("div", { class: "empty" }, message);
}

// ---------------------------------------------------------------- state

const state = {
  stores: [],
  theme: "light",
  navOpen: window.innerWidth >= 900,
};

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

// ---------------------------------------------------------------- views

async function storesView() {
  const { stores } = await api("/api/stores");
  state.stores = stores;

  const totals = stores.reduce(
    (sum, store) => ({
      files: sum.files + store.files,
      chunks: sum.chunks + store.chunks,
      bytes: sum.bytes + store.bytes,
    }),
    { files: 0, chunks: 0, bytes: 0 },
  );

  const stat = (key, value, sub) =>
    el(
      "div",
      { class: "stat" },
      el("span", { class: "eyebrow", text: key }),
      el("span", { class: "value", text: value }),
      sub ? el("span", { class: "sub", text: sub }) : null,
    );

  const rows = stores.map((store) =>
    el(
      "tr",
      {},
      el(
        "td",
        {},
        el("div", { style: "font-weight:600", text: store.name }),
        el("div", { class: "meta mono", style: "font-size:11px", text: store.dir }),
      ),
      el(
        "td",
        { class: "meta" },
        store.roots.length
          ? store.roots.map((root) =>
              el(
                "div",
                { title: root.present ? "" : "this root is not on disk" },
                root.present ? root.path : `${root.path} — missing`,
              ),
            )
          : "—",
      ),
      el("td", { class: "num", text: n(store.files) }),
      el("td", { class: "num", text: n(store.chunks) }),
      el(
        "td",
        {},
        el(
          "span",
          { class: `pill ${store.watching ? "good" : ""}` },
          el("span", { class: `dot ${store.watching ? "live" : "idle"}` }),
          store.watching ? when(store.last_write) : "not watching",
        ),
      ),
    ),
  );

  const feed = stores
    .flatMap((store) => store.events.map((event) => ({ ...event, store: store.name })))
    .sort((a, b) => a.at - b.at)
    .slice(-40);

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
        el("p", {
          class: "subtitle",
          text: "Every store this daemon holds the write lock on.",
        }),
      ),
      el(
        "div",
        { class: "buttons" },
        el("button", { class: "button", type: "button", onclick: () => go("index") }, icon(ICONS.plus), "Index a folder"),
      ),
    ),
    el(
      "div",
      { class: "strip" },
      stat("Stores", n(stores.length)),
      stat("Files", n(totals.files)),
      stat("Chunks", n(totals.chunks)),
      stat("On disk", bytes(totals.bytes)),
      stat(
        "Watching",
        `${stores.filter((s) => s.watching).length}/${stores.length}`,
        "roots kept current",
      ),
    ),
    stores.length
      ? el(
          "div",
          { class: "card", style: "overflow:hidden" },
          el(
            "div",
            { class: "table-wrap" },
            el(
              "table",
              { style: "min-width:640px" },
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
                ),
              ),
              el("tbody", {}, rows),
            ),
          ),
        )
      : error("No store is open. Index a folder to make one."),
    el(
      "div",
      { class: "grid" },
      el(
        "div",
        { class: "card pad" },
        el(
          "div",
          { style: "display:flex;align-items:baseline;gap:8px;margin-bottom:12px" },
          el("strong", { text: "Watcher" }),
          el("span", { class: "muted mono", style: "font-size:11px", text: "live" }),
        ),
        feed.length
          ? el(
              "div",
              { class: "feed" },
              feed.map((event) =>
                el(
                  "div",
                  { class: "row" },
                  el("span", { class: "at", text: clock(event.at) }),
                  el("span", {}, `${event.store}: ${event.text}`),
                ),
              ),
            )
          : el("p", { class: "subtitle", text: "Nothing has changed since the daemon started." }),
      ),
      el(
        "div",
        { class: "card pad" },
        el("strong", { text: "Model" }),
        el(
          "div",
          { class: "rows", style: "margin-top:12px" },
          stores.map((store) =>
            el(
              "div",
              { class: "kv" },
              el("span", { class: "k", text: store.name }),
              el("span", { class: "v", text: `${store.model} · ${store.dim} dim` }),
            ),
          ),
        ),
      ),
    ),
  );
}

async function filesView() {
  const controls = { path: "", ext: new Set(), store: new Set() };
  const table = el("div", { class: "card", style: "overflow:hidden" });
  const count = el("span", { class: "muted mono", style: "font-size:12px" });

  async function load() {
    const params = new URLSearchParams();
    if (controls.path) params.append("path", controls.path);
    for (const ext of controls.ext) params.append("ext", ext);
    for (const store of controls.store) params.append("store", store);

    fill(table, el("div", { class: "view", style: "padding:20px" }, "Loading…"));
    let data;
    try {
      data = await api(`/api/files?${params}`);
    } catch (e) {
      fill(table, error(e.message));
      return;
    }

    count.textContent = `${n(data.total)} files${
      data.total > data.files.length ? ` · showing the first ${n(data.files.length)}` : ""
    }`;

    if (!data.files.length) {
      fill(table, error("Nothing indexed matches that filter."));
      return;
    }

    fill(
      table,
      el(
        "div",
        { class: "table-wrap" },
        el(
          "table",
          { style: "min-width:620px" },
          el(
            "thead",
            {},
            el(
              "tr",
              {},
              el("th", { text: "Path" }),
              el("th", { text: "Store" }),
              el("th", { text: "Reader" }),
              el("th", { class: "num", text: "Lines" }),
              el("th", { class: "num", text: "Chunks" }),
              el("th", { text: "" }),
            ),
          ),
          el(
            "tbody",
            {},
            data.files.map((file) =>
              el(
                "tr",
                {},
                el("td", { class: "path", text: file.path }),
                el("td", { class: "meta", text: file.store }),
                el("td", {}, el("span", { class: "tag", text: file.reader })),
                el("td", { class: "num", text: n(file.lines) }),
                el("td", { class: "num", text: n(file.chunks) }),
                el(
                  "td",
                  {},
                  el("button", {
                    class: "forget",
                    type: "button",
                    text: "Forget",
                    onclick: async () => {
                      try {
                        await post("/api/forget", { path: file.path, store: file.store });
                        await load();
                      } catch (e) {
                        alert(e.message);
                      }
                    },
                  }),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  const pathInput = el("input", {
    type: "search",
    placeholder: "path glob, e.g. src/**",
    oninput: (e) => {
      controls.path = e.target.value.trim();
      clearTimeout(load.timer);
      load.timer = setTimeout(load, 220);
    },
  });

  const EXTENSIONS = ["rs", "md", "py", "ts", "js", "go", "json", "pdf", "ipynb", "docx", "epub", "rtf", "eml"];
  const extChips = EXTENSIONS.map((ext) =>
    el("button", {
      class: "chip",
      type: "button",
      "aria-pressed": "false",
      text: `.${ext}`,
      onclick: (e) => {
        const on = e.currentTarget.getAttribute("aria-pressed") === "true";
        e.currentTarget.setAttribute("aria-pressed", on ? "false" : "true");
        if (on) controls.ext.delete(ext);
        else controls.ext.add(ext);
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
        el("p", { class: "subtitle", text: "What is indexed, and which reader parsed it." }),
      ),
      count,
    ),
    el(
      "div",
      { class: "filters" },
      el("div", { class: "field", style: "width:280px" }, icon(ICONS.search), pathInput),
      extChips,
    ),
    table,
    el("p", {
      class: "subtitle",
      text: "Forget drops the file's chunks and vectors. The file on disk is untouched.",
    }),
  );
}

// Every name `--lang` accepts and what makes it up.
//
// The table is the filter's own, served from /api/languages, so what is shown
// here is what a search would actually have matched rather than a second list
// that drifts. Two of the names have no extension at all — a Dockerfile and a
// Makefile are known by being called that — which is why filenames get their
// own column instead of being written as if they were extensions.
async function languagesView() {
  let data;
  try {
    data = await api("/api/languages");
  } catch (e) {
    return el("div", { class: "view" }, error(e.message));
  }
  const languages = data.languages || [];

  const rows = languages.map((language) =>
    el(
      "tr",
      {},
      el("td", { class: "path" }, language.name),
      el(
        "td",
        { class: "meta" },
        (language.extensions || []).map((ext) => el("span", { class: "tag", text: `.${ext}` })),
      ),
      el(
        "td",
        { class: "meta" },
        (language.filenames || []).length
          ? language.filenames.map((name) => el("span", { class: "tag", text: name }))
          : el("span", { class: "muted", text: "—" }),
      ),
    ),
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
        el("h1", { text: "Languages" }),
        el("p", {
          class: "subtitle",
          text: "What --lang accepts on the command line, in the MCP tools and in the search box.",
        }),
      ),
      el("span", { class: "pill", text: `${languages.length} languages` }),
    ),
    el(
      "div",
      { class: "card" },
      el(
        "div",
        { class: "table-wrap" },
        el(
          "table",
          {},
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
          el("tbody", {}, rows),
        ),
      ),
    ),
    el("p", {
      class: "subtitle",
      text: "Extension and filename decide the language; file contents are never read to guess it, because a store is searched far more often than it is built.",
    }),
  );
}

async function searchView() {
  const results = el("div", { class: "results" });
  const meta = el("span", { class: "search-meta" });
  const chosen = new Set();

  const input = el("input", {
    type: "search",
    placeholder: "Ask it something",
    onkeydown: (e) => {
      if (e.key === "Enter") run();
    },
  });

  async function run() {
    const query = input.value.trim();
    if (!query) {
      fill(results, el("div", { class: "empty" }, "Type a phrase or an identifier."));
      meta.textContent = "";
      return;
    }
    const params = new URLSearchParams({ query, k: "8" });
    for (const store of chosen) params.append("store", store);

    meta.textContent = "searching…";
    let data;
    try {
      data = await api(`/api/search?${params}`);
    } catch (e) {
      fill(results, error(e.message));
      meta.textContent = "";
      return;
    }

    meta.textContent = `${data.hits.length} of ${n(data.chunks)} chunks · ${(
      data.micros / 1000
    ).toFixed(1)} ms`;

    if (!data.hits.length) {
      fill(
        results,
        el(
          "div",
          { class: "empty" },
          "No chunk in the selected stores matches that. The filter, not the corpus — clear a store chip and ask again.",
        ),
      );
      return;
    }

    fill(
      results,
      data.hits.map((hit, i) =>
        el(
          "div",
          { class: `hit ${i === 0 ? "best" : ""}` },
          el(
            "div",
            { class: "where" },
            el("span", { text: hit.path }),
            el("span", { class: "lines", text: `:${hit.start_line}-${hit.end_line}` }),
            hit.store ? el("span", { class: "from", text: hit.store }) : null,
            el("span", { class: "spacer" }),
            el("span", { class: "from", text: hit.score.toFixed(4) }),
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
        const on = e.currentTarget.getAttribute("aria-pressed") === "true";
        e.currentTarget.setAttribute("aria-pressed", on ? "false" : "true");
        if (on) chosen.delete(store.name);
        else chosen.add(store.name);
        run();
      },
    }),
  );

  fill(results, el("div", { class: "empty" }, "Type a phrase or an identifier."));
  setTimeout(() => input.focus(), 0);

  return el(
    "div",
    { style: "display:flex;flex-direction:column;min-height:100%" },
    el(
      "div",
      { class: "search-band" },
      el("div", { class: "search-field" }, icon(ICONS.search), input, meta),
      el(
        "div",
        { class: "filters" },
        storeChips.length ? storeChips : el("span", { class: "muted", text: "no store open" }),
        el("span", { class: "spacer" }),
        el("span", {
          class: "muted mono",
          style: "font-size:11px",
          text: "vector + fts5, fused by rank",
        }),
      ),
    ),
    results,
  );
}

async function indexView() {
  const field = el("input", { type: "text", placeholder: "~/work/api" });
  const log = el("pre", { class: "log" });
  const bar = el("span", { style: "width:0%" });
  const picker = el("div", { class: "card", hidden: true });
  const start = el("button", { class: "button", type: "button", text: "Start indexing" });
  // No example URL in the placeholder: every byte the portal serves is checked
  // to name no origin but its own, and a literal link here reads as one.
  const urlField = el("input", { type: "text", placeholder: "link to a page, a PDF or a file" });
  const addButton = el("button", {
    class: "button secondary",
    type: "button",
    text: "Fetch and index",
  });

  const storeSelect = el(
    "select",
    { class: "chip static", style: "font-family:inherit" },
    state.stores.map((store) => el("option", { value: store.name, text: store.name })),
  );

  function say(text, key) {
    const line = el("div", {}, key ? el("span", { class: "key", text: `${key} ` }) : null, text);
    log.append(line);
    log.scrollTop = log.scrollHeight;
  }

  async function browse(path) {
    let data;
    try {
      data = await api(`/api/dirs${path ? `?path=${encodeURIComponent(path)}` : ""}`);
    } catch (e) {
      alert(e.message);
      return;
    }
    picker.hidden = false;
    fill(
      picker,
      el(
        "div",
        {
          style:
            "padding:14px 18px;border-bottom:1px solid var(--line);display:flex;align-items:center;gap:10px;flex-wrap:wrap",
        },
        el("span", { class: "mono", style: "font-size:13px;font-weight:600", text: data.path }),
        el("span", { class: "spacer" }),
        data.parent
          ? el("button", {
              class: "button secondary small",
              type: "button",
              text: "Up",
              onclick: () => browse(data.parent),
            })
          : null,
        el("button", {
          class: "button small",
          type: "button",
          text: "Use this folder",
          onclick: () => {
            field.value = data.path;
            picker.hidden = true;
          },
        }),
      ),
      el(
        "div",
        { class: "picker" },
        data.entries.length
          ? data.entries.map((entry) =>
              el(
                "button",
                {
                  class: "entry",
                  type: "button",
                  onclick: () => (entry.dir ? browse(entry.path) : (field.value = entry.path)),
                },
                icon(entry.dir ? ICONS.folder : ICONS.file),
                entry.name,
                el("span", { class: "kind", text: entry.dir ? "folder" : "file" }),
              ),
            )
          : el("div", { style: "padding:14px 18px", class: "muted", text: "Nothing here." }),
      ),
    );
  }

  // One reader for both buttons. /api/add fetches the URL and then hands what
  // landed to the same write queue a folder goes through, so it answers with
  // the same event stream and there is no second progress mechanism to keep in
  // step with this one.
  async function run(route, body) {
    start.disabled = true;
    addButton.disabled = true;
    log.replaceChildren();
    say(`POST ${route} · newline-delimited JSON, chunked`, "→");
    bar.style.width = "0%";

    try {
      const response = await fetch(route, {
        method: "POST",
        credentials: "same-origin",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ ...body, store: storeSelect.value || undefined }),
      });
      if (!response.ok) throw new Error((await response.text()) || response.statusText);

      // The response is chunked and arrives while the writer works, so it is
      // read as it comes rather than awaited whole — which is the entire point
      // of streaming it.
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
            const done = event.total ? (event.scanned / event.total) * 100 : 0;
            bar.style.width = `${Math.min(100, done).toFixed(1)}%`;
            say(event.path, `${event.scanned}/${event.total}`);
          } else if (event.event === "done") {
            bar.style.width = "100%";
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
          } else if (event.event === "error") {
            say(event.error, "error");
          } else if (event.event === "started") {
            say(event.paths.join(", "), "indexing");
          }
        }
      }
    } catch (e) {
      say(e.message, "error");
    } finally {
      start.disabled = false;
      addButton.disabled = false;
    }
  }

  start.addEventListener("click", () => {
    const path = field.value.trim();
    if (!path) {
      alert("Give a path to index.");
      return;
    }
    run("/api/index", { path });
  });

  addButton.addEventListener("click", () => {
    const url = urlField.value.trim();
    if (!url) {
      alert("Give an https URL to fetch.");
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
        { class: "field tall", style: "flex:1 1 240px;max-width:420px" },
        el("span", { class: "prefix", text: "path" }),
        field,
      ),
      el("button", {
        class: "button secondary",
        type: "button",
        text: "Choose folder…",
        onclick: () => browse(""),
      }),
      state.stores.length > 1 ? storeSelect : null,
      start,
      el("span", { class: "spacer" }),
      el("span", {
        class: "muted mono",
        style: "font-size:11px",
        text: "writer: daemon",
      }),
    ),
    picker,
    el(
      "div",
      { class: "card pad" },
      el("div", { class: "eyebrow", text: "Add from a URL" }),
      el("p", {
        class: "subtitle",
        style: "margin:6px 0 12px",
        text: "One https request, for exactly this URL — a page, a PDF, or a file on GitHub. Nothing is crawled and no credential is ever sent. The file is saved inside this store's downloads folder, never in your working tree.",
      }),
      el(
        "div",
        { class: "filters", style: "margin:0" },
        el(
          "div",
          { class: "field tall", style: "flex:1 1 240px;max-width:420px" },
          el("span", { class: "prefix", text: "url" }),
          urlField,
        ),
        addButton,
      ),
    ),
    el(
      "div",
      { class: "card pad" },
      el("div", { class: "eyebrow", text: "Progress" }),
      el("div", { class: "bar", style: "margin-top:12px" }, bar),
    ),
    log,
  );
}

// The install one-liners, what `semlith setup` would report about this machine,
// and the only two places semlith ever reaches out for a new version. It is
// one card because the welcome screen and the Agents view owe the same answer,
// and it reads `/api/setup`, which is `setup::status()` — the same function the
// terminal prints, so the page cannot claim PATH is set up when it is not.
function installPanel() {
  const card = el("div", { class: "card pad" });
  const head = () => el("strong", { text: "Install and setup" });
  fill(card, head(), el("p", { class: "subtitle", text: "Checking this machine…" }));

  const stateWord = {
    done: "done",
    "already-done": "already done",
    skipped: "not done",
    failed: "failed",
  };

  api("/api/setup")
    .then((setup) => {
      const result = el("p", { class: "subtitle" });

      const check = el("button", {
        class: "button secondary small",
        type: "button",
        text: "Check for updates",
        onclick: async () => {
          result.textContent = "Asking GitHub…";
          try {
            const found = await post("/api/upgrade", { action: "check" });
            result.textContent = found.available
              ? `${found.latest} is available; you are on ${found.installed}.`
              : `Already on the newest release, ${found.installed}.`;
            if (found.blocked) result.textContent += ` ${found.blocked}`;
            install.disabled = !found.available || Boolean(found.blocked);
          } catch (e) {
            result.textContent = e.message;
          }
        },
      });

      const install = el("button", {
        class: "button small",
        type: "button",
        text: "Install it",
        onclick: async () => {
          install.disabled = true;
          result.textContent = "Downloading and verifying…";
          try {
            const done = await post("/api/upgrade", { action: "apply" });
            result.textContent = done.restart;
          } catch (e) {
            result.textContent = e.message;
          }
        },
      });
      install.disabled = true;

      fill(
        card,
        head(),
        el("p", {
          class: "subtitle",
          text: "One command on a new machine. Nothing here runs until you click it.",
        }),
        el("div", { class: "eyebrow", style: "margin-top:12px", text: "macOS and Linux" }),
        copyField(setup.install_sh),
        el("div", { class: "eyebrow", style: "margin-top:12px", text: "Windows" }),
        copyField(setup.install_ps1),
        el("div", { class: "rule", style: "margin:14px 0" }),
        el(
          "div",
          { class: "rows" },
          setup.steps.map((step) =>
            el(
              "div",
              { style: "display:flex;gap:10px;align-items:baseline" },
              el("span", {
                class: `pill ${step.state === "done" || step.state === "already-done" ? "good" : ""}`,
                text: stateWord[step.state] || step.state,
              }),
              el("span", { style: "font-weight:600", text: step.name }),
              el("span", { class: "mono muted", style: "font-size:12px", text: step.detail }),
            ),
          ),
        ),
        el("div", {
          class: "subtitle",
          style: "margin-top:10px",
          text: setup.on_path
            ? `${setup.bin_dir} is on PATH.`
            : `${setup.bin_dir} is not on PATH — run \`semlith setup\` to add it.`,
        }),
        el("div", { class: "rule", style: "margin:14px 0" }),
        el("div", { class: "eyebrow", text: `Installed version ${setup.version}` }),
        el("div", { class: "buttons", style: "margin-top:8px" }, check, install),
        result,
      );
    })
    .catch((e) => fill(card, head(), error(e.message)));

  return card;
}

async function agentsView() {
  const data = await api("/api/agents");
  const clients = data.clients;
  const body = el("div", { class: "card pad", style: "display:flex;flex-direction:column;gap:14px" });
  let current = 0;

  function show(i) {
    current = i;
    const client = clients[i];
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
      el("div", { style: "font-weight:600", text: client.name }),
      el("p", { class: "subtitle", text: client.note }),
      client.stanzas.map((stanza) =>
        el(
          "div",
          { style: "display:flex;flex-direction:column;gap:8px" },
          el("div", { class: "eyebrow", text: stanza.format }),
          el("pre", { class: "code", text: stanza.text.trimEnd() }),
          el("div", { class: "buttons" }, copyButton(stanza.text.trimEnd(), "Copy stanza")),
        ),
      ),
    );
  }
  show(current);

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
        el("span", { class: `dot ${data.forwarding ? "live" : "idle"}` }),
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
        el("strong", { text: "Tools exposed" }),
        el(
          "div",
          { class: "rows", style: "margin-top:12px" },
          data.tools.map((tool) => el("div", { class: "mono", style: "font-size:12.5px", text: tool })),
        ),
        el("div", { class: "rule", style: "margin:14px 0" }),
        el("div", { class: "eyebrow", text: "Protocol revisions" }),
        el("div", {
          class: "mono muted",
          style: "font-size:12px;margin-top:6px",
          text: data.revisions.join(" · "),
        }),
      ),
      body,
      installPanel(),
    ),
  );
}

async function privacyView() {
  const data = await api("/api/privacy");
  const tokenBox = el("div", {
    class: "copyfield",
    style: "background:var(--bg)",
  });

  function showToken(text) {
    fill(tokenBox, el("code", { text }));
  }
  showToken("held by this browser's cookie, not shown");

  const fact = (key, value, why) =>
    el(
      "div",
      { class: "stat", style: "flex:1 1 230px" },
      el("span", { class: "eyebrow", text: key }),
      el("span", { class: "mono", style: "font-size:14px", text: value }),
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
          { style: "display:flex;align-items:center;gap:10px;flex-wrap:wrap" },
          el("h1", { text: "Privacy" }),
          el(
            "span",
            { class: `pill ${data.airgap ? "good" : ""}` },
            data.airgap ? "airgap mode armed" : "airgap off",
          ),
        ),
        el("p", {
          class: "subtitle",
          text: "Everything on this page is checkable in about a minute. None of it is a promise you have to take.",
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
      fact("Telemetry", "none", "no analytics, no update check, no outbound call"),
      fact(
        "Model cache",
        data.model_cached ? "cached" : "not downloaded",
        data.model_cache,
      ),
    ),
    el(
      "div",
      { class: "grid" },
      el(
        "div",
        { class: "card pad", style: "display:flex;flex-direction:column;gap:14px" },
        el("strong", { text: "Verify it with a packet capture" }),
        el("p", {
          class: "subtitle",
          text: "Watch every interface but loopback while you search. Nothing should appear.",
        }),
        copyField("sudo tcpdump -i any -n 'not host 127.0.0.1 and not host ::1'"),
        el("p", {
          class: "subtitle",
          text: "Or pull the cable: the portal loads and searches with no network at all.",
        }),
        el("div", { class: "rule" }),
        el("strong", { text: "The one outbound connection that exists" }),
        el("p", {
          class: "subtitle",
          text: "The embedding model is downloaded once, on first index, and cached. --airgap refuses even that and exits naming the cache path.",
        }),
        copyField("semlith index . --airgap"),
      ),
      el(
        "div",
        { class: "card pad", style: "display:flex;flex-direction:column;gap:14px" },
        el("strong", { text: "Session token" }),
        el("p", {
          class: "subtitle",
          text: `One token per run of the daemon, held in a SameSite=Strict ${data.token_cookie} cookie. Rotating it invalidates the old one immediately.`,
        }),
        tokenBox,
        el(
          "div",
          { class: "buttons" },
          el("button", {
            class: "button secondary small",
            type: "button",
            text: "Rotate",
            onclick: async () => {
              try {
                const fresh = await post("/api/rotate", {});
                showToken(fresh.token);
              } catch (e) {
                alert(e.message);
              }
            },
          }),
        ),
        el("div", { class: "rule" }),
        el("strong", { text: "Content-Security-Policy" }),
        el("pre", { class: "code", text: data.csp }),
        el("p", {
          class: "subtitle",
          text: `Host headers answered: ${data.host_allowed.join(", ")}. Everything else gets 400.`,
        }),
      ),
    ),
  );
}

async function aboutView() {
  const [about, models] = await Promise.all([api("/api/about"), api("/api/models")]);
  const row = (key, value) =>
    el("div", { class: "kv" }, el("span", { class: "k", text: key }), el("span", { class: "v", text: value }));

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
        { class: "card pad", style: "display:flex;flex-direction:column;gap:10px" },
        row("Version", about.version),
        row("Binary", about.binary),
        row("Bound", `127.0.0.1:${about.port}`),
        row("Pid", String(about.pid)),
        row("Uptime", `${about.uptime}s`),
        row("Store home", about.store_home),
        row("Model cache", about.model_cache),
        row("Stores open", String(about.stores)),
        row("Languages", String(about.languages)),
      ),
      el(
        "div",
        { class: "card", style: "overflow:hidden" },
        el(
          "div",
          { class: "table-wrap" },
          el(
            "table",
            { style: "min-width:480px" },
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
              models.models.map((model) =>
                el(
                  "tr",
                  {},
                  el("td", { class: "path", text: model.name }),
                  el("td", { class: "num", text: String(model.dim) }),
                  el("td", { class: "subtitle", text: model.description }),
                ),
              ),
            ),
          ),
        ),
      ),
    ),
    el("p", {
      class: "subtitle",
      text: "A store keeps the model it was built with: vectors from two models are not comparable, so changing it means a re-index.",
    }),
  );
}

function welcomeView() {
  const field = el("input", { type: "text", placeholder: "~/work/api" });
  const note = el("p", { class: "subtitle" });

  return el(
    "div",
    { class: "welcome" },
    el(
      "div",
      { class: "lockup" },
      el("span", { class: "logo", text: "s" }),
      el("span", { class: "name", text: "semlith" }),
    ),
    el(
      "div",
      { class: "hero" },
      el("h2", { text: "No stores yet" }),
      el("p", {
        text: "Point semlith at a folder and it reads every file in it once, then keeps it current as you save. Nothing leaves this machine.",
      }),
      el("div", { class: "field tall" }, el("span", { class: "prefix", text: "~/" }), field),
      el(
        "div",
        { class: "buttons" },
        el("button", {
          class: "button",
          type: "button",
          text: "Index this folder",
          onclick: async () => {
            const path = field.value.trim();
            if (!path) {
              note.textContent = "Give a path first.";
              return;
            }
            go("index");
          },
        }),
        el("button", {
          class: "button secondary",
          type: "button",
          text: "Adopt an existing .semlith",
          onclick: async () => {
            const path = field.value.trim();
            if (!path) {
              note.textContent = "Give the path of the .semlith directory to adopt.";
              return;
            }
            try {
              const done = await post("/api/adopt", { path });
              note.textContent = `Adopted as ${done.name} at ${done.dir}. Restart the daemon to open it.`;
            } catch (e) {
              note.textContent = e.message;
            }
          },
        }),
        el("button", {
          class: "button ghost",
          type: "button",
          text: "Skip for now",
          onclick: () => go("stores"),
        }),
      ),
      note,
      el("div", { class: "rule" }),
      el("div", { class: "eyebrow", text: "Or from a terminal" }),
      copyField("semlith index ~/work/api"),
    ),
    el(
      "div",
      { class: "steps" },
      [
        ["01", "Index", "One pass over the folder, then only what changes."],
        ["02", "Start", "semlith start owns the stores and serves this page."],
        ["03", "Point an agent at it", "semlith mcp, with no path to keep in step."],
      ].map(([num, title, text]) =>
        el(
          "div",
          { class: "step" },
          el("span", { class: "eyebrow", text: num }),
          el("strong", { text: title }),
          el("span", { class: "subtitle", text }),
        ),
      ),
    ),
    installPanel(),
    el("footer", { text: "127.0.0.1 · loopback only · no external asset" }),
  );
}

// ---------------------------------------------------------------- shell

const RENDER = {
  stores: storesView,
  files: filesView,
  search: searchView,
  languages: languagesView,
  index: indexView,
  agents: agentsView,
  privacy: privacyView,
  about: aboutView,
};

function go(id) {
  location.hash = `#${id}`;
}

function theme(next) {
  state.theme = next;
  document.documentElement.setAttribute("data-theme", next);
  try {
    localStorage.setItem("semlith-theme", next);
  } catch (_) {
    // A private window refuses storage. The toggle still works for this page;
    // it just will not be remembered, which is not worth failing over.
  }
}

function chrome(current) {
  const groups = [];
  for (const view of VIEWS) {
    let group = groups.find((g) => g.name === view.group);
    if (!group) groups.push((group = { name: view.group, items: [] }));
    group.items.push(view);
  }

  const sidebar = el(
    "nav",
    { class: "sidebar", hidden: !state.navOpen },
    groups.map((group) => [
      el("div", { class: "nav-group-label", text: group.name }),
      group.items.map((view) =>
        el("button", {
          class: "nav-item",
          type: "button",
          text: view.label,
          "aria-current": view.id === current ? "page" : null,
          onclick: () => {
            if (window.innerWidth < 900) state.navOpen = false;
            go(view.id);
          },
        }),
      ),
    ]),
    el(
      "div",
      { class: "daemon-card" },
      el(
        "div",
        { style: "display:flex;align-items:center;gap:8px" },
        el("span", { class: "dot live" }),
        el("strong", { text: "semlith start" }),
      ),
      el("span", { class: "mono", text: `${location.host} · sole writer` }),
      el("span", {
        class: "mono",
        text: `${state.stores.length} store${state.stores.length === 1 ? "" : "s"}`,
      }),
    ),
  );

  const scrim = el("button", {
    class: `scrim ${state.navOpen ? "open" : ""}`,
    type: "button",
    "aria-label": "Close navigation",
    onclick: () => {
      state.navOpen = false;
      render();
    },
  });

  const main = el("main", {});

  const shell = el(
    "div",
    { id: "app" },
    el(
      "header",
      { class: "topbar" },
      el(
        "button",
        {
          class: "hamburger",
          type: "button",
          "aria-label": "Toggle navigation",
          onclick: () => {
            state.navOpen = !state.navOpen;
            render();
          },
        },
        icon(ICONS.menu),
      ),
      el("span", { class: "logo", text: "s" }),
      el("span", { class: "wordmark", text: "semlith" }),
      el("span", { class: "divider-v" }),
      el("span", {
        class: "page-title",
        text: (VIEWS.find((v) => v.id === current) || {}).title || "",
      }),
      el("span", { class: "spacer" }),
      el(
        "button",
        { class: "search-launcher", type: "button", onclick: () => go("search") },
        icon(ICONS.search),
        el("span", { text: "Search the corpus" }),
        el("span", { class: "key-hint", text: "/" }),
      ),
      el(
        "button",
        {
          class: "icon-button",
          type: "button",
          "aria-label": "Toggle theme",
          onclick: () => theme(state.theme === "dark" ? "light" : "dark"),
        },
        icon(ICONS.sun),
      ),
    ),
    el("div", { class: "shell" }, sidebar, scrim, main),
  );

  return { shell, main };
}

async function render() {
  const root = document.getElementById("root");
  const current = (location.hash || "#stores").slice(1);

  // The welcome screen is the whole page, not a view inside the shell: with no
  // store there is nothing for the navigation to navigate.
  if (!state.stores.length && current !== "index" && current !== "about") {
    fill(root, welcomeView());
    return;
  }

  const { shell, main } = chrome(current);
  fill(root, shell);
  fill(main, el("div", { class: "view" }, "Loading…"));

  const view = RENDER[current] || RENDER.stores;
  try {
    fill(main, await view());
  } catch (e) {
    fill(main, el("div", { class: "view" }, error(e.message)));
  }
}

async function boot() {
  try {
    theme(localStorage.getItem("semlith-theme") || "light");
  } catch (_) {
    theme("light");
  }

  // The token arrived in the URL and the server has turned it into a cookie.
  // Taking it out of the address bar keeps it out of the browser's history and
  // out of anything the user pastes when they share the link.
  if (new URLSearchParams(location.search).has("token")) {
    history.replaceState({}, "", location.pathname + location.hash);
  }

  try {
    const { stores } = await api("/api/stores");
    state.stores = stores;
  } catch (e) {
    fill(document.getElementById("root"), el("div", { class: "view" }, error(e.message)));
    return;
  }

  window.addEventListener("hashchange", render);
  document.addEventListener("keydown", (e) => {
    if (e.key === "/" && !/^(INPUT|TEXTAREA|SELECT)$/.test(document.activeElement.tagName)) {
      e.preventDefault();
      go("search");
    }
  });
  render();
}

boot();
