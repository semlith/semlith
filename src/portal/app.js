/* semlith portal
 *
 * Plain DOM, no framework, no build step. The whole file is served from the
 * binary, so there is nothing to bundle and nothing to fetch: what you read
 * here is what runs.
 *
 * Laid out after design v6 (2026-10-01): a Welcome screen and a store wizard
 * outside the shell, and inside it nine pages in three groups — Home, Stores
 * (with one page per store), Search, Graph, Agents, Ledger, Reports, Privacy,
 * Settings. Every view is a function that builds an element from the data
 * cache; the router swaps one for another inside <main>. Parts of a view that
 * move with live data are painted in place, so a field being typed into is
 * never rebuilt under the cursor.
 *
 * The server's policy is `style-src 'self'`: a `style` attribute is dropped
 * without a word, so nothing here writes one. Classes come from style.css and
 * data-driven sizes go through the CSSOM (`node.style.width = …`).
 */

"use strict";

// ------------------------------------------------------------------ helpers

/** Build an element. Attributes in `props`, children as the rest. */
function el(tag, props, ...kids) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(props || {})) {
    if (value === null || value === undefined || value === false) continue;
    if (key === "class") node.className = value;
    // Always its own text node, even empty, so a redraw patched in place
    // edits the words rather than adding or removing a node.
    else if (key === "text") node.append(document.createTextNode(String(value)));
    else if (key.startsWith("on") && typeof value === "function") listen(node, key.slice(2), value);
    else node.setAttribute(key, value === true ? "" : String(value));
  }
  for (const kid of kids.flat(Infinity)) {
    if (kid === null || kid === undefined || kid === false) continue;
    node.append(kid.nodeType ? kid : document.createTextNode(String(kid)));
  }
  // Every drawn checkbox carries its tick as an inline SVG, shown when it is
  // checked: the design's tick, with no image URL for the CSP to refuse.
  if (node.classList.contains("cb")) {
    const tick = icon("M5 12.5l4.5 4.5L19 7.5", 12, { w: 3.2 });
    tick.setAttribute("class", "i cb-tick");
    node.append(tick);
  }
  return node;
}

/* Handlers go through one listener per event that reads the node's current
 * handler, so a redraw patched in place (morph) can hand an old node the new
 * closure without adding a second listener. */
function listen(node, type, fn) {
  if (!node.__h) node.__h = {};
  if (!(type in node.__h)) node.addEventListener(type, (e) => node.__h[type] && node.__h[type](e));
  node.__h[type] = fn;
}

/* Patch the live tree a into the freshly drawn b, keeping every node whose
 * place and tag still match: a pressed button stays the same button, a field
 * being typed in keeps its text and focus, a scrolled box keeps its offset.
 * A subtree marked data-morph-keep is left alone (a log something else is
 * filling). Only for views whose drawing is synchronous: a view that fills a
 * node later holds the new node, which this throws away. */
function morph(a, b) {
  if (a.nodeType !== b.nodeType || (a.nodeType === 1 && a.tagName !== b.tagName)) {
    a.replaceWith(b);
    return;
  }
  if (a.nodeType !== 1) {
    if (a.data !== b.data) a.data = b.data;
    return;
  }
  const keep = a.getAttribute("data-morph-keep");
  if (keep && keep === b.getAttribute("data-morph-keep")) return;
  for (const { name } of [...a.attributes]) if (!b.hasAttribute(name)) a.removeAttribute(name);
  for (const { name, value } of [...b.attributes]) if (a.getAttribute(name) !== value) a.setAttribute(name, value);
  if ("value" in b && a !== document.activeElement && a.value !== b.value) a.value = b.value;
  if ("checked" in b && a.checked !== b.checked) a.checked = b.checked;
  for (const [type, fn] of Object.entries(b.__h || {})) listen(a, type, fn);
  if (a.__h) for (const type of Object.keys(a.__h)) if (!b.__h || !(type in b.__h)) a.__h[type] = null;
  const ak = [...a.childNodes];
  const bk = [...b.childNodes];
  for (let i = 0; i < bk.length; i++) {
    if (i < ak.length) morph(ak[i], bk[i]);
    else a.append(bk[i]);
  }
  for (let i = bk.length; i < ak.length; i++) ak[i].remove();
}

/** A button. Every clickable thing in the portal is one, so Tab reaches it. */
function btn(props, ...kids) {
  const p = { type: "button", ...props };
  // A control whose work takes a moment says so: while the promise its
  // handler returns is pending it is marked busy, shows a spinner and ignores
  // presses, so a switch that waits on the daemon is never pressed twice or
  // taken for broken. Not `disabled`: disabling the focused button drops a
  // keyboard user's focus to the page. Synchronous handlers are untouched.
  if (typeof p.onclick === "function") {
    const handler = p.onclick;
    p.onclick = (e) => {
      const node = e.currentTarget;
      if (node?.classList.contains("busy")) return;
      const out = handler(e);
      if (out && typeof out.then === "function" && node) {
        node.classList.add("busy");
        node.setAttribute("aria-busy", "true");
        node.setAttribute("aria-disabled", "true");
        const done = () => {
          node.classList.remove("busy");
          node.removeAttribute("aria-busy");
          node.removeAttribute("aria-disabled");
        };
        out.then(done, done);
      }
      return out;
    };
  }
  return el("button", p, ...kids);
}

/** Replace an element's children. */
function fill(node, ...kids) {
  node.replaceChildren();
  for (const kid of kids.flat(Infinity)) {
    if (kid === null || kid === undefined || kid === false) continue;
    node.append(kid.nodeType ? kid : document.createTextNode(String(kid)));
  }
  return node;
}

/* Write words into a node a live poll keeps rewriting, editing its one text
 * node only when the words moved — a replaced child is re-announced by a live
 * region and loses a selection. */
function setText(node, value) {
  const text = value === null || value === undefined ? "" : String(value);
  const only = node.firstChild;
  if (only && only === node.lastChild && only.nodeType === Node.TEXT_NODE) {
    if (only.data !== text) only.data = text;
  } else if (node.textContent !== text) {
    node.textContent = text;
  }
  return node;
}

/** A bar whose fill is `pct` of its track. The width is set through the CSSOM. */
function bar(pct, cls) {
  const fillNode = el("i");
  fillNode.style.width = `${Math.max(0, Math.min(100, pct || 0)).toFixed(1)}%`;
  return el("div", { class: cls ? `bar ${cls}` : "bar" }, fillNode);
}

/** Move a bar made by `bar` to a new value without rebuilding it. */
function setBar(node, pct) {
  const fillNode = node.firstElementChild;
  if (fillNode) fillNode.style.width = `${Math.max(0, Math.min(100, pct || 0)).toFixed(1)}%`;
}

/* The session token. It arrives once in the URL the daemon printed, moves into
 * this page and sessionStorage, and is taken out of the address bar at once —
 * never a cookie, because every port on localhost is the same site. */
const TOKEN_HEADER = "Semlith-Token";

const session = (() => {
  const KEY = "semlith.token";
  let token = "";
  try {
    const fromUrl = new URLSearchParams(location.search).get("token");
    if (fromUrl) {
      token = fromUrl;
      sessionStorage.setItem(KEY, token);
      history.replaceState(null, "", location.pathname + location.hash || "/");
    } else {
      token = sessionStorage.getItem(KEY) || "";
    }
  } catch (_) {
    /* storage refused: the token lives as long as this document */
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

function authed(extra) {
  return { ...(extra || {}), [TOKEN_HEADER]: session.get() };
}

/** True once a request has failed to reach the daemon at all. */
let daemonDown = false;

// Requests on their way, by path, with when each left: what the browser drive
// reports when a page never finishes loading.
const INFLIGHT = (window.__semlithInflight = new Map());

async function api(path, options) {
  const key = `${((options || {}).method || "GET").toUpperCase()} ${path}#${Math.random().toString(36).slice(2, 6)}`;
  INFLIGHT.set(key, performance.now());
  try {
    return await fetchJson(path, options);
  } finally {
    INFLIGHT.delete(key);
  }
}

async function fetchJson(path, options) {
  const options_ = options || {};
  let response;
  try {
    response = await fetch(path, { credentials: "omit", ...options_, headers: authed(options_.headers) });
  } catch (e) {
    noteDaemon(false);
    throw new Error("The daemon is not answering. Is `semlith start` still running?");
  }
  noteDaemon(true);
  if (!response.ok) {
    let detail = response.statusText;
    try {
      const body = await response.json();
      if (body && body.error) detail = body.error;
    } catch (_) {
      /* 401 sends an empty body on purpose */
    }
    if (response.status === 401) detail = "This tab's session token is not the daemon's. Open the portal from the URL `semlith start` printed.";
    const error = new Error(detail || "request failed");
    error.status = response.status;
    throw error;
  }
  const type = response.headers.get("Content-Type") || "";
  if (type.includes("application/json")) return response.json();
  return response.blob();
}

function post(path, body) {
  return api(path, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body || {}),
  });
}

const NUM = new Intl.NumberFormat("en-US");
const n = (value) => NUM.format(Math.round(Number(value) || 0));
const plural = (count, one, many) => `${n(count)} ${count === 1 ? one : many || `${one}s`}`;

function pct(part, whole, digits) {
  if (!whole) return "0%";
  const value = (part / whole) * 100;
  if (digits !== undefined) return `${value.toFixed(digits)}%`;
  return `${value < 10 && value > 0 ? value.toFixed(1) : Math.round(value)}%`;
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

/** Tokens in the short form the design uses: 1.2M, 384k, 912. */
function short(value) {
  const v = Math.abs(Number(value) || 0);
  const sign = Number(value) < 0 ? "−" : "";
  if (v >= 1e9) return `${sign}${(v / 1e9).toFixed(1)}B`;
  if (v >= 1e6) return `${sign}${(v / 1e6).toFixed(1)}M`;
  if (v >= 1e4) return `${sign}${Math.round(v / 1e3)}k`;
  if (v >= 1e3) return `${sign}${(v / 1e3).toFixed(1)}k`;
  return `${sign}${Math.round(v)}`;
}

function dollars(value) {
  const v = Number(value) || 0;
  if (v === 0) return "$0";
  if (Math.abs(v) < 0.01) return "<$0.01";
  return `$${v.toFixed(v >= 100 ? 0 : 2)}`;
}

function zone() {
  const minutes = -new Date().getTimezoneOffset();
  const sign = minutes < 0 ? "-" : "+";
  const off = Math.abs(minutes);
  return `${sign}${String(Math.floor(off / 60)).padStart(2, "0")}:${String(off % 60).padStart(2, "0")}`;
}

/** How long ago, in the words the design uses: "just now", "4m ago". */
function ago(unix) {
  if (!unix) return "never";
  const seconds = Math.max(0, Math.floor(Date.now() / 1000) - unix);
  if (seconds < 45) return "just now";
  if (seconds < 3600) return `${Math.max(1, Math.round(seconds / 60))}m ago`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
  if (seconds < 86400 * 30) return `${Math.floor(seconds / 86400)}d ago`;
  return new Date(unix * 1000).toISOString().slice(0, 10);
}

/** A clock time with its offset, so a portal time and a ledger row agree. */
function clock(unix) {
  return new Date(unix * 1000).toTimeString().slice(0, 5);
}

function clockFull(unix) {
  return `${new Date(unix * 1000).toTimeString().slice(0, 8)} ${zone()}`;
}

function dayClock(unix) {
  const d = new Date(unix * 1000);
  const today = new Date();
  const same = d.toDateString() === today.toDateString();
  const yesterday = new Date(today.getTime() - 86400000).toDateString() === d.toDateString();
  const time = d.toTimeString().slice(0, 5);
  if (same) return `today ${time}`;
  if (yesterday) return `yesterday ${time}`;
  return `${d.toISOString().slice(5, 10)} ${time}`;
}

function spellTook(ms) {
  if (ms < 1000) return `${(ms / 1000).toFixed(1)}s`;
  const all = Math.round(ms / 1000);
  if (all < 60) return `${all}s`;
  if (all < 3600) return `${Math.floor(all / 60)}m ${String(all % 60).padStart(2, "0")}s`;
  return `${Math.floor(all / 3600)}h ${String(Math.floor(all / 60) % 60).padStart(2, "0")}m`;
}

function spellLeft(ms) {
  if (ms === null || ms === undefined) return "estimating…";
  const seconds = Math.round(ms / 1000);
  if (seconds < 10) return "almost done";
  if (seconds < 60) return `${Math.round(seconds / 5) * 5}s left`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 90) return `${minutes} min left`;
  return `${Math.floor(minutes / 60)} h ${String(minutes % 60).padStart(2, "0")} min left`;
}

function perSecond(value) {
  return value >= 10 ? n(Math.round(value)) : Number(value || 0).toFixed(1);
}

/** The tail of a path, for a store root in a narrow column. */
function tilde(path, home) {
  const p = String(path || "");
  const h = home || state.home;
  if (h && p.startsWith(h)) return `~${p.slice(h.length)}`;
  return p;
}

// A long path, shortened for the page: home as ~, then the middle cut so the
// start (where it lives) and the end (what it is) both stay. The full path
// goes in the element's tooltip; see pathEl.
// A long path keeps its end, which is the part that tells two apart: the
// start is cut, at a folder boundary where one is near.
function shortPath(path, max) {
  const t = tilde(path);
  const limit = max || 52;
  if (t.length <= limit) return t;
  let tail = t.slice(t.length - (limit - 1));
  const cut = tail.search(/[\\/]/);
  if (cut > 0 && cut < tail.length / 3) tail = tail.slice(cut);
  return `…${tail}`;
}

/** Any path-shaped text as a span that, when it does not fit, loses its start
 * rather than its end; the whole value is on hover. */
function pathSpan(text, cls, tip) {
  return el("span", { class: `${cls ? `${cls} ` : ""}ell-start`, "data-tip": String(tip ?? text) }, el("bdi", { text: String(text) }));
}

/** A path as a span: cut from the start to fit, with the whole of it on hover. */
function pathEl(path, max, cls) {
  const short = shortPath(path, max);
  return el("span", { class: `${cls || "t-mono-sm"} ell-start`, "data-tip": String(path) }, el("bdi", { text: short }));
}

// Every absolute path inside a sentence, shortened the same way.
function shortPaths(text, max) {
  return String(text || "").replace(/(?:[A-Za-z]:\\|\/)[^\s,;'")]+/g, (p) => shortPath(p, max || 44));
}

function baseName(path) {
  const parts = String(path).split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] || path;
}

function stillness() {
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

async function copy(text, word) {
  try {
    await navigator.clipboard.writeText(text);
  } catch (_) {
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
  toast(word || "Copied");
}

function download(name, data, type) {
  const blob = data instanceof Blob ? data : new Blob([data], { type: type || "text/plain" });
  const url = URL.createObjectURL(blob);
  const a = el("a", { href: url, download: name });
  document.body.append(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 2000);
}

// --------------------------------------------------------------------- icons

const SVG_NS = "http://www.w3.org/2000/svg";

/** An inline icon, stroked in the current colour. Decorative. */
function icon(d, size, opts) {
  const o = opts || {};
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("width", String(size || 14));
  svg.setAttribute("height", String(size || 14));
  svg.setAttribute("class", "i");
  svg.setAttribute("fill", o.solid ? "currentColor" : "none");
  svg.setAttribute("stroke", o.solid ? "none" : "currentColor");
  svg.setAttribute("stroke-width", String(o.w || 1.7));
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("aria-hidden", "true");
  for (const segment of String(d).split("|")) {
    if (segment.startsWith("circle:")) {
      const [cx, cy, r] = segment.slice(7).split(",");
      const c = document.createElementNS(SVG_NS, "circle");
      c.setAttribute("cx", cx);
      c.setAttribute("cy", cy);
      c.setAttribute("r", r);
      svg.append(c);
      continue;
    }
    const path = document.createElementNS(SVG_NS, "path");
    path.setAttribute("d", segment);
    svg.append(path);
  }
  return svg;
}

const I = {
  home: "M4 11 12 4l8 7v9h-5v-6H9v6H4Z",
  stores: "M12 3 3 7.5 12 12l9-4.5L12 3ZM3 12l9 4.5 9-4.5M3 16.5 12 21l9-4.5",
  search: "M10.5 4a6.5 6.5 0 1 0 0 13 6.5 6.5 0 0 0 0-13ZM20 20l-4.8-4.8",
  searchSm: "circle:11,11,7|m20 20-3.5-3.5",
  graph: "M4 5h4v4H4ZM15 15h5v5h-5ZM6 9v4h5v4h4",
  agents: "M5 11h14v9H5ZM9 3h6v5H9ZM12 8v3M9 15h.01M15 15h.01",
  ledger: "M5 3h14v18H5ZM9 8h6M9 12h6M9 16h4",
  reports: "M14 3H7a1.5 1.5 0 0 0-1.5 1.5v15A1.5 1.5 0 0 0 7 21h10a1.5 1.5 0 0 0 1.5-1.5V7.5ZM14 3v4.5h4.5M9.5 17v-3M12 17v-5M14.5 17v-2",
  privacy: "M12 3 5 6v5c0 4.5 3 8.3 7 10 4-1.7 7-5.5 7-10V6Z",
  settings: "M4 7h10M18 7h2M4 12h4M12 12h8M4 17h12M20 17h0M14 5v4M8 10v4M16 15v4",
  menu: "M4 7h16M4 12h16M4 17h16",
  plus: "M12 5v14M5 12h14",
  arrow: "M5 12h14M13 6l6 6-6 6",
  back: "M19 12H5M11 6l-6 6 6 6",
  check: "M5 12.5l4.5 4.5L19 7",
  x: "M6 6l12 12M18 6 6 18",
  folder: "M3 6.5A1.5 1.5 0 0 1 4.5 5H9l2 2.5h8.5A1.5 1.5 0 0 1 21 9v8.5a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 17.5Z",
  file: "M14 3H7a1.5 1.5 0 0 0-1.5 1.5v15A1.5 1.5 0 0 0 7 21h10a1.5 1.5 0 0 0 1.5-1.5V7.5ZM14 3v4.5h4.5",
  link: "M10 14a4 4 0 0 0 5.66 0l3-3a4 4 0 0 0-5.66-5.66l-1 1M14 10a4 4 0 0 0-5.66 0l-3 3a4 4 0 0 0 5.66 5.66l1-1",
  paste: "M9 4h6v3H9ZM7 5.5H5.5v15h13v-15H17M9 12h6M9 16h4",
  upload: "M12 16V4M7 9l5-5 5 5M4 16v3a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1v-3",
  download: "M12 4v11M7 10l5 5 5-5M5 20h14",
  info: "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18ZM12 11v6M12 7.5v.5",
  alert: "M12 3 2 20h20ZM12 10v4M12 17v.5",
  more: "circle:12,5,1.8|circle:12,12,1.8|circle:12,19,1.8",
  chevDown: "m6 9 6 6 6-6",
  chevRight: "m9 6 6 6-6 6",
  chevLeft: "m15 6-6 6 6 6",
  clock: "circle:12,12,8|M12 8v4l2.5 2",
  swap: "M7 7h12l-3-3M17 17H5l3 3",
  copy: "M8 8h12v12H8ZM16 8V5a1 1 0 0 0-1-1H5a1 1 0 0 0-1 1v10a1 1 0 0 0 1 1h3",
  sun: "M12 8.5a3.5 3.5 0 1 0 0 7 3.5 3.5 0 0 0 0-7ZM12 2.5v2M12 19.5v2M2.5 12h2M19.5 12h2M5.3 5.3l1.4 1.4M17.3 17.3l1.4 1.4M5.3 18.7l1.4-1.4M17.3 6.7l1.4-1.4",
  moon: "M20 14.5A8 8 0 0 1 9.5 4a8 8 0 1 0 10.5 10.5Z",
  system: "M3.5 5h17v11h-17ZM9 20h6M12 16v4",
  shield: "M12 3 5 6v5c0 4.5 3 8.3 7 10 4-1.7 7-5.5 7-10V6Z",
  layers: "M12 3 3 7.5 12 12l9-4.5L12 3ZM3 12l9 4.5 9-4.5M3 16.5 12 21l9-4.5",
};

// ------------------------------------------------------------- primitives

function pill(text, tone, opts) {
  const o = opts || {};
  return el(
    "span",
    { class: `pill ${tone || "grey"}${o.sm ? " sm" : ""}`, "data-tip": o.tip || null },
    o.dot === false ? null : el("span", { class: `dot ${dotTone(tone)}${o.pulse ? " pulse" : ""}` }),
    text,
  );
}

function dotTone(tone) {
  return { green: "green", amber: "amber", blue: "blue", red: "red" }[tone] || "";
}

function dot(tone, pulse) {
  return el("span", { class: `dot ${dotTone(tone)}${pulse ? " pulse" : ""}` });
}

/** An on / off switch: the design's track and knob, as a real button. */
function toggle(on, label, onChange, opts) {
  const o = opts || {};
  const node = btn(
    {
      class: `switch${o.cls ? ` ${o.cls}` : ""}${on ? " on" : ""}`,
      role: "switch",
      "aria-checked": String(!!on),
      disabled: o.disabled || null,
      "data-tip": o.tip || null,
      onclick: () => onChange(!on),
    },
    el("span", { class: "tg" }),
    label || null,
  );
  return node;
}

/** The design's toggle row: a bordered card with a switch, a title and a line. */
function toggleRow(on, title, sub, onChange, opts) {
  const o = opts || {};
  return btn(
    {
      class: `${o.plain ? "tg-plain" : "tg-row"}${on ? " on" : ""}`,
      role: "switch",
      "aria-checked": String(!!on),
      disabled: o.disabled || null,
      "data-tip": o.tip || null,
      onclick: () => onChange(!on),
    },
    el("span", { class: "tg" }),
    el("span", { class: "col" }, el("span", { class: `t${o.big ? " big" : ""}`, text: title }), sub ? el("span", { class: "s", text: sub }) : null),
  );
}

/** A drawn checkbox. `state` is true, false or "mixed". */
function checkbox(stateValue, onChange, label) {
  return btn({
    class: "cb",
    role: "checkbox",
    "aria-checked": stateValue === "mixed" ? "mixed" : String(!!stateValue),
    "aria-label": label || "Select",
    onclick: (e) => {
      e.stopPropagation();
      onChange(stateValue !== true);
    },
  });
}

/** A segmented control. `items` are `[value, label, count?]`. */
function seg(items, current, onPick, opts) {
  const o = opts || {};
  return el(
    "div",
    { class: `seg${o.cls ? ` ${o.cls}` : ""}`, role: "group", "aria-label": o.label || null },
    items.map(([value, label, count, tip]) =>
      btn(
        {
          "aria-pressed": String(value === current),
          "data-tip": tip || null,
          disabled: o.disabled && o.disabled(value) ? true : null,
          onclick: () => onPick(value),
        },
        label,
        count !== undefined && count !== null ? el("span", { class: "n", text: String(count) }) : null,
      ),
    ),
  );
}

/** Tabs along a rule. `items` are `[id, label, count?]`. */
function tabs(items, current, onPick, opts) {
  const o = opts || {};
  return el(
    "div",
    { class: `tabs${o.cls ? ` ${o.cls}` : ""}`, role: "tablist" },
    items.map(([id, label, count, tone]) =>
      btn(
        { class: "tab", role: "tab", "aria-selected": String(id === current), onclick: () => onPick(id) },
        label,
        count !== undefined && count !== null && count !== "" ? el("span", { class: `count${tone ? ` ${tone}` : ""}`, text: String(count) }) : null,
      ),
    ),
    o.extra || null,
  );
}

function kpi(label, value, sub, opts) {
  const o = opts || {};
  const tag = o.onclick ? "button" : "div";
  const node = el(
    tag,
    { class: `kpi${o.warn ? " warn" : ""}`, type: o.onclick ? "button" : null, onclick: o.onclick || null, "data-tip": o.tip || null, "data-tip-rows": o.rows || null },
    el("span", { class: "eyebrow", text: label }),
    el("span", { class: "v", text: value }),
    sub ? el("span", { class: "s", text: sub }) : null,
  );
  return node;
}

function cardHead(title, ...extra) {
  return el("div", { class: "card-h" }, el("span", { class: "card-t grow", text: title }), ...extra);
}

function meta(text) {
  return el("span", { class: "card-meta", text });
}

function lnk(text, onclick, cls) {
  return btn({ class: `lnk${cls ? ` ${cls}` : ""}`, onclick }, text);
}

function empty(text, cls) {
  return el("div", { class: `empty${cls ? ` ${cls}` : ""}` }, text);
}

function errorBox(message) {
  return el("div", { class: "error-box", role: "alert", text: message });
}

function copyBtn(getText, label, cls, word) {
  return btn(
    {
      class: `btn ${cls || "xs"}`,
      onclick: () => copy(typeof getText === "function" ? getText() : getText, word),
    },
    label || "Copy",
  );
}

/** A command or path in a field with a Copy button beside it. */
function copyField(text, opts) {
  const o = opts || {};
  return el(
    "div",
    { class: `copyfield${o.cls ? ` ${o.cls}` : ""}` },
    el("span", { class: "t", text }),
    ...(o.actions || []),
    o.noCopy ? null : copyBtn(text, "Copy", o.btn || "xxs"),
  );
}

function codeBlock(text, opts) {
  const o = opts || {};
  return el(
    "div",
    { class: `code${o.cls ? ` ${o.cls}` : ""}` },
    text,
    o.noCopy ? null : btn({ class: "copy-on", onclick: () => copy(text, o.word) }, "Copy"),
  );
}

// ------------------------------------------------------------------- tooltip

/* One rich tooltip for the page. An element carries `data-tip` (the title) and
 * optionally `data-tip-rows` ("label::value||label::value") and
 * `data-tip-color` (a swatch). It follows the pointer, eased, and hangs off a
 * focused element for the keyboard. The rows are built as nodes rather than
 * HTML, because a `style` attribute in markup would be dropped. */
const tip = {
  node: null,
  card: null,
  cur: null,
  shown: false,
  x: 0,
  y: 0,
  tx: 0,
  ty: 0,
  raf: 0,
  timer: 0,
  last: null,

  ensure() {
    if (this.node) return;
    this.lab = el("span", { class: "lab" });
    this.sw = el("span", { class: "sw", hidden: true });
    this.rows = el("div", { class: "rows", hidden: true });
    this.card = el("div", { class: "card-in" }, el("div", { class: "hd" }, this.sw, this.lab), this.rows);
    this.node = el("div", { class: "tip", role: "tooltip", "aria-hidden": "true" }, this.card);
    document.body.append(this.node);
  },

  find(target) {
    const host = target && target.closest ? target.closest("[data-tip]") : null;
    if (!host) return null;
    const title = host.getAttribute("data-tip");
    if (!title) return null;
    return {
      el: host,
      title,
      rows: host.getAttribute("data-tip-rows") || "",
      color: host.getAttribute("data-tip-color") || "",
      full: host.hasAttribute("data-tip-full"),
    };
  },

  render(f) {
    this.ensure();
    this.lab.textContent = f.title;
    this.lab.className = f.full ? "lab full" : "lab";
    this.sw.hidden = !f.color;
    if (f.color) this.sw.style.background = f.color;
    const rows = f.rows ? f.rows.split("||").filter(Boolean).map((r) => r.split("::")) : [];
    this.rows.hidden = !rows.length;
    fill(
      this.rows,
      rows.map(([k, v]) => el("div", { class: "rw" }, el("span", { class: "k", text: k }), el("span", { class: "v", text: v || "" }))),
    );
  },

  place(e) {
    if (!this.cur || !e) return;
    const w = this.card.offsetWidth;
    const h = this.card.offsetHeight;
    const W = window.innerWidth;
    const H = window.innerHeight;
    let x = e.clientX + 14;
    let y = e.clientY + 20;
    let ox = "left";
    let oy = "top";
    if (x + w > W - 8) {
      x = e.clientX - w - 14;
      ox = "right";
    }
    if (y + h > H - 8) {
      y = e.clientY - h - 14;
      oy = "bottom";
    }
    x = Math.max(8, Math.min(W - w - 8, x));
    y = Math.max(8, Math.min(H - h - 8, y));
    this.tx = x;
    this.ty = y;
    this.card.style.transformOrigin = `${oy} ${ox}`;
    if (!this.shown || stillness()) {
      this.x = x;
      this.y = y;
      this.node.style.transform = `translate3d(${x}px,${y}px,0)`;
      if (!this.shown) {
        this.shown = true;
        this.node.style.opacity = "1";
        this.card.style.transform = "scale(1)";
      }
      return;
    }
    if (!this.raf) {
      const step = () => {
        const dx = this.tx - this.x;
        const dy = this.ty - this.y;
        this.x += dx * 0.28;
        this.y += dy * 0.28;
        if (Math.abs(dx) < 0.3 && Math.abs(dy) < 0.3) {
          this.x = this.tx;
          this.y = this.ty;
          this.raf = 0;
        } else this.raf = requestAnimationFrame(step);
        this.node.style.transform = `translate3d(${this.x.toFixed(1)}px,${this.y.toFixed(1)}px,0)`;
      };
      this.raf = requestAnimationFrame(step);
    }
  },

  /** For focus, which has no pointer: under the element, or above it. */
  at(target) {
    const f = this.find(target);
    if (!f) return;
    this.cur = f;
    this.render(f);
    const r = f.el.getBoundingClientRect();
    this.place({ clientX: r.left, clientY: r.bottom - 12 });
  },

  hide() {
    clearTimeout(this.timer);
    if (!this.cur) return;
    this.cur = null;
    this.shown = false;
    cancelAnimationFrame(this.raf);
    this.raf = 0;
    if (this.node) {
      this.node.style.opacity = "0";
      this.card.style.transform = "scale(0.97)";
    }
  },

  wire() {
    document.addEventListener(
      "pointerover",
      (e) => {
        if (graphDragging) return;
        const f = this.find(e.target);
        if (!f) {
          this.hide();
          return;
        }
        const c = this.cur;
        if (c && c.el === f.el && c.title === f.title && c.rows === f.rows) return;
        clearTimeout(this.timer);
        this.last = e;
        const show = () => {
          this.cur = f;
          this.render(f);
          this.place(this.last || e);
        };
        if (c) show();
        else this.timer = setTimeout(() => f.el.isConnected && show(), 220);
      },
      true,
    );
    document.addEventListener(
      "pointermove",
      (e) => {
        this.last = { clientX: e.clientX, clientY: e.clientY };
        if (!this.cur) return;
        if (!this.cur.el.isConnected) return this.hide();
        this.place(e);
      },
      { passive: true },
    );
    document.addEventListener("pointerdown", () => this.hide(), true);
    document.addEventListener("focusin", (e) => {
      if (e.target.matches(":focus-visible")) this.at(e.target);
    });
    document.addEventListener("focusout", () => this.hide());
    window.addEventListener("scroll", () => this.hide(), true);
  },
};

/** `data-tip-rows` from pairs, skipping empty values. */
function rows(pairs) {
  return pairs
    .filter(([, v]) => v !== null && v !== undefined && v !== "")
    .map(([k, v]) => `${k}::${v}`)
    .join("||");
}

// ------------------------------------------------------------- menu, modal

const menu = {
  node: null,
  owner: null,
  ensure() {
    if (this.node) return this.node;
    this.node = el("div", { class: "menu", role: "menu", hidden: true });
    document.body.append(this.node);
    document.addEventListener("pointerdown", (e) => {
      if (this.node.hidden) return;
      if (this.node.contains(e.target) || (this.owner && this.owner.contains(e.target))) return;
      this.close();
    });
    document.addEventListener("keydown", (e) => {
      // Up and down move through the items, as they do in a system list.
      if ((e.key === "ArrowDown" || e.key === "ArrowUp") && !this.node.hidden) {
        const items = [...this.node.querySelectorAll(".menu-item:not([disabled])")];
        const i = items.indexOf(document.activeElement);
        const next = items[(i + (e.key === "ArrowDown" ? 1 : -1) + items.length) % items.length];
        if (next) {
          next.focus();
          e.preventDefault();
        }
      }
      if (e.key === "Escape" && !this.node.hidden) {
        const owner = this.owner;
        this.close();
        if (owner) owner.focus();
      }
    });
    window.addEventListener("resize", () => this.close());
    window.addEventListener("scroll", (e) => !this.node.contains(e.target) && this.close(), true);
    return this.node;
  },
  /** `items` are `{ label, hint, tone, checked, onclick }` or null for none. */
  open(at, items, opts) {
    const node = this.ensure();
    if (this.owner === at && !node.hidden) return this.close();
    this.owner = at;
    at.setAttribute("aria-expanded", "true");
    fill(
      node,
      items.filter(Boolean).map((item) =>
        btn(
          {
            class: `menu-item${item.tone ? ` ${item.tone}` : ""}`,
            role: item.checked !== undefined ? "menuitemradio" : "menuitem",
            "aria-checked": item.checked !== undefined ? String(!!item.checked) : null,
            disabled: item.disabled || null,
            onclick: () => {
              this.close();
              item.onclick();
            },
          },
          el("span", { class: "ell", text: item.label }),
          item.hint ? el("span", { class: "hint", text: item.hint }) : null,
        ),
      ),
    );
    node.style.minWidth = `${(opts && opts.width) || 200}px`;
    node.hidden = false;
    const box = node.getBoundingClientRect();
    const rect = at.getBoundingClientRect();
    let left = opts && opts.alignLeft ? rect.left : rect.right - box.width;
    left = Math.min(Math.max(8, left), window.innerWidth - 8 - box.width);
    let top = rect.bottom + 6;
    if (top + box.height > window.innerHeight - 8) top = rect.top - box.height - 6;
    node.style.left = `${Math.round(left)}px`;
    node.style.top = `${Math.round(Math.max(8, top))}px`;
    const first = node.querySelector(".menu-item[aria-checked='true']") || node.querySelector(".menu-item");
    if (first) first.focus();
  },
  close() {
    if (!this.node) return;
    this.node.hidden = true;
    if (this.owner) this.owner.setAttribute("aria-expanded", "false");
    this.owner = null;
  },
};

/** The portal's dropdown, drawn like the Graph's store picker (a button with
 * a thin chevron, and the menu): in place of the system <select>, which draws
 * differently on every OS and cannot be styled to match. `options` are
 * `[value, label, hint?]`; `cls` sizes it (sm by default, beside 28px
 * fields). The chosen value is on `data-value`. */
function dropdown({ label, value, options, onChange, cls, width }) {
  const cur = options.find((o) => String(o[0]) === String(value)) || options[0] || ["", ""];
  const b = btn(
    {
      class: `btn dd ${cls || "sm"}`,
      "aria-haspopup": "menu",
      "aria-expanded": "false",
      "aria-label": label,
      "data-value": String(cur[0]),
      onclick: (e) => {
        e.stopPropagation();
        menu.open(
          b,
          options.map(([v, text, hint]) => ({ label: text, hint, checked: String(v) === String(b.getAttribute("data-value")), onclick: () => {
            b.setAttribute("data-value", String(v));
            b.firstChild.textContent = text;
            onChange(v);
          } })),
          { width: width || Math.max(160, b.offsetWidth), alignLeft: true },
        );
      },
    },
    el("span", { class: "ell", text: cur[1] }),
    icon(I.chevDown, 12, { w: 2 }),
  );
  return b;
}

/** The row menu's three-dot control. */
function moreButton(items, label) {
  const b = btn(
    {
      class: "btn icon",
      "aria-haspopup": "menu",
      "aria-expanded": "false",
      "aria-label": label || "Actions",
      "data-tip": label || "Actions",
      onclick: (e) => {
        e.stopPropagation();
        menu.open(b, items());
      },
    },
    icon(I.more, 13, { solid: true }),
  );
  return b;
}

/* The confirm modal. `ask` resolves true on OK and false on Cancel/Escape.
 * `extra` is a node shown under the body — a "also delete" option, a list. */
function ask({ title, body, ok, cancel, danger, extra, wide }) {
  return new Promise((resolve) => {
    const before = document.activeElement;
    const keep = before && before.getAttribute ? before.getAttribute("data-keep") : null;
    const done = (value) => {
      scrim.remove();
      document.removeEventListener("keydown", onKey, true);
      // Back to the control that opened it, or its redrawn copy.
      const back = before && before.isConnected ? before : keep ? document.querySelector(`[data-keep="${keep}"]`) : null;
      if (back && back.focus) back.focus();
      resolve(value);
    };
    const onKey = (e) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        done(false);
      }
    };
    const okButton = btn({ class: `btn ${danger ? "danger" : "primary"}`, onclick: () => done(true) }, ok || "OK");
    const modal = el(
      "div",
      { class: `modal${wide ? " wide" : ""}`, role: "dialog", "aria-modal": "true", "aria-label": title },
      el("div", { class: "mt", text: title }),
      body ? el("div", { class: "mb" }, body) : null,
      extra || null,
      el("div", { class: "acts" }, btn({ class: "btn", onclick: () => done(false) }, cancel || "Cancel"), okButton),
    );
    // Closed by its buttons or Escape only: a click outside, or one that
    // starts inside (selecting the path, say) and ends outside, is not an
    // answer.
    const scrim = el("div", { class: "modal-scrim" }, modal);
    document.body.append(scrim);
    document.addEventListener("keydown", onKey, true);
    okButton.focus();
  });
}

let toastTimer = 0;
function toast(message, bad) {
  document.querySelectorAll(".toast").forEach((t) => t.remove());
  const node = el(
    "div",
    { class: `toast${bad ? " bad" : ""}${state.screen === "wizard" ? " up" : ""}`, role: "status", "aria-live": "polite" },
    icon(bad ? I.x : I.check, 14, { w: 2.6 }),
    el("span", { text: message }),
  );
  document.body.append(node);
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => node.remove(), bad ? 5200 : 2800);
}

/** Run a write, toast its failure, and hand back its answer or null. */
async function act(fn, okMessage) {
  try {
    const out = await fn();
    if (okMessage) toast(typeof okMessage === "function" ? okMessage(out) : okMessage);
    return out;
  } catch (e) {
    toast(e.message, true);
    return null;
  }
}

// --------------------------------------------------------------------- grid

/* One table for the portal: sortable headers with an arrow, a rows-per-page
 * select, a numbered pager, and multi-select with "select all N matching".
 * `spec`:
 *   key        remembers sort, page and size across repaints
 *   columns    [{ key, label, cls, sort (row→value), render (row→node), head }]
 *   rows       every row (client-side) — or `total` + `onQuery` for server side
 *   id         row → stable id, needed for selection
 *   select     true to show checkboxes; `canSelect(row)` narrows it
 *   actions    sel → nodes for the selection bar
 *   empty      text when nothing matches, `onClear` adds "Clear the filters"
 *   foot       extra text before the pager
 *   per        default page size
 */
const GRID_VIEWS = new Map();

function grid(spec) {
  const view = GRID_VIEWS.get(spec.key) || { page: 1, per: spec.per || 10, sort: spec.sort || null, dir: spec.dir || "asc", sel: new Set(), all: false };
  if (spec.key) GRID_VIEWS.set(spec.key, view);
  const node = el("div", { class: "grid-wrap" });
  let rowsIn = spec.rows || [];

  function sorted() {
    if (spec.server || !view.sort) return rowsIn;
    const column = spec.columns.find((c) => c.key === view.sort);
    if (!column || !column.sort) return rowsIn;
    const sign = view.dir === "desc" ? -1 : 1;
    return [...rowsIn].sort((a, b) => {
      const x = column.sort(a);
      const y = column.sort(b);
      if (typeof x === "number" && typeof y === "number") return (x - y) * sign;
      return String(x ?? "").localeCompare(String(y ?? ""), undefined, { numeric: true }) * sign;
    });
  }
  const total = () => (spec.server ? spec.total || 0 : rowsIn.length);
  const pages = () => Math.max(1, Math.ceil(total() / view.per));
  const selectable = (row) => spec.select && (!spec.canSelect || spec.canSelect(row));

  function changed() {
    if (spec.server && spec.onQuery) spec.onQuery({ page: view.page, per: view.per, sort: view.sort, dir: view.dir });
    paint();
  }

  function paint() {
    view.page = Math.min(view.page, pages());
    const all = sorted();
    const shown = spec.server ? all : all.slice((view.page - 1) * view.per, view.page * view.per);
    const shownSelectable = shown.filter(selectable);
    const allIds = all.filter(selectable).map((r) => spec.id(r));
    // A selection is of rows still in the set: a filter that removes a row
    // removes it from what a bulk action would touch.
    if (!spec.server) for (const id of [...view.sel]) if (!allIds.includes(id)) view.sel.delete(id);
    const pageState = shownSelectable.length && shownSelectable.every((r) => view.sel.has(spec.id(r))) ? true : shownSelectable.some((r) => view.sel.has(spec.id(r))) ? "mixed" : false;
    const head = el(
      "tr",
      {},
      spec.select
        ? el(
            "th",
            { class: "cbx" },
            checkbox(pageState, (on) => {
              for (const r of shownSelectable) on ? view.sel.add(spec.id(r)) : view.sel.delete(spec.id(r));
              view.all = false;
              paint();
            }, "Select this page"),
          )
        : null,
      spec.columns.map((c) => {
        const thCls = (c.cls || "").split(" ").filter((x) => x === "r" || x === "cbx").join(" ") || null;
        if (!c.sort) return el("th", { class: thCls, text: c.label || "" });
        const on = view.sort === c.key;
        return el(
          "th",
          { class: thCls, "aria-sort": on ? (view.dir === "asc" ? "ascending" : "descending") : "none" },
          btn(
            {
              class: "sort",
              onclick: () => {
                if (view.sort === c.key) view.dir = view.dir === "asc" ? "desc" : "asc";
                else {
                  view.sort = c.key;
                  view.dir = c.firstDir || "asc";
                }
                view.page = 1;
                changed();
              },
            },
            c.label,
            el("span", { class: "ar", text: on ? (view.dir === "asc" ? "↑" : "↓") : "" }),
          ),
        );
      }),
    );
    const body = el(
      "tbody",
      {},
      shown.map((row, i) => {
        const id = spec.select ? spec.id(row) : null;
        const on = id !== null && view.sel.has(id);
        return el(
          "tr",
          { class: on ? "sel" : null },
          spec.select
            ? el(
                "td",
                { class: "cbx" },
                selectable(row)
                  ? checkbox(on, (v) => {
                      v ? view.sel.add(id) : view.sel.delete(id);
                      view.all = false;
                      paint();
                    })
                  : null,
              )
            : null,
          spec.columns.map((c) => el("td", { class: c.cls || null }, c.render ? c.render(row, i) : String(c.sort ? c.sort(row) ?? "" : ""))),
        );
      }),
    );
    const count = view.sel.size;
    // A server-paged grid holds one page; "all matching" is the whole result
    // set, which the bulk action reads again with the same filters.
    const matching = spec.server ? total() : allIds.length;
    const selbar =
      spec.select && count
        ? el(
            "div",
            { class: "selbar" },
            el("span", { class: "what", text: view.all ? `All ${n(matching)} matching selected` : `${n(count)} selected` }),
            !view.all && matching > count
              ? lnk(`Select all ${n(matching)} matching`, () => {
                  allIds.forEach((x) => view.sel.add(x));
                  view.all = true;
                  paint();
                }, "strong")
              : null,
            lnk("Clear selection", () => {
              view.sel.clear();
              view.all = false;
              paint();
            }, "muted"),
            el("span", { class: "spacer" }),
            el("div", { class: "row gap6" }, spec.actions ? spec.actions([...view.sel], () => {
              view.sel.clear();
              view.all = false;
              paint();
            }, { all: view.all, total: matching }) : null),
          )
        : null;
    const from = total() ? (view.page - 1) * view.per + 1 : 0;
    const to = Math.min(total(), view.page * view.per);
    fill(
      node,
      selbar,
      el("div", { class: "tw" }, el("table", { class: spec.cls || null }, spec.caption ? el("caption", { class: "sr-only", text: spec.caption }) : null, el("thead", {}, head), body)),
      // A server-paged grid is waiting on its first answer until it has one:
      // "No file matches" there read as an empty store.
      !total() && spec.loading
        ? el("div", { class: "empty lg tb row gap10", role: "status" }, el("span", { class: "spinner" }), spec.loadingText || "Loading…")
        : !total()
          ? el("div", { class: "empty lg tb" }, spec.empty || "Nothing here yet.", spec.onClear ? [" ", lnk("Clear the filters", spec.onClear)] : null)
          : null,
      spec.noFoot
        ? null
        : el(
            "div",
            { class: "card-foot" },
            el("span", { class: "grow", text: spec.loading && !total() ? "reading…" : `${n(from)}–${n(to)} of ${n(total())}${spec.foot && total() ? ` · ${spec.foot}` : ""}` }),
            pager(view, pages(), changed),
          ),
    );
  }

  if (spec.server && spec.loading === undefined) spec.loading = true;
  paint();
  return {
    node,
    update(next, totalCount) {
      rowsIn = next || [];
      if (totalCount !== undefined) spec.total = totalCount;
      spec.loading = false;
      node.classList.remove("is-loading");
      paint();
    },
    // A new query is on its way: rows already shown stay, dimmed, rather than
    // the table emptying under the reader.
    loading() {
      spec.loading = true;
      node.classList.add("is-loading");
      if (!rowsIn.length) paint();
    },
    selected: () => [...view.sel],
    // Every row the filters let through, in the order on screen: what an
    // export writes, so a file says what the table said.
    shown: () => sorted(),
    clear() {
      view.sel.clear();
      paint();
    },
    view,
  };
}

/** Rows-per-page and a numbered pager, as the design draws it under every table. */
function pager(view, pages, changed) {
  const go = (p) => {
    view.page = Math.max(1, Math.min(pages, p));
    changed();
  };
  const nums = [];
  const add = (p) => nums.push(btn({ class: "pg", "aria-current": p === view.page ? "page" : null, onclick: () => go(p) }, String(p)));
  if (pages <= 7) for (let p = 1; p <= pages; p++) add(p);
  else {
    add(1);
    const lo = Math.max(2, view.page - 1);
    const hi = Math.min(pages - 1, view.page + 1);
    if (lo > 2) nums.push(el("span", { class: "gap", text: "…" }));
    for (let p = lo; p <= hi; p++) add(p);
    if (hi < pages - 1) nums.push(el("span", { class: "gap", text: "…" }));
    add(pages);
  }
  const select = dropdown({
    label: "Rows per page",
    value: view.per,
    options: [10, 25, 50, 100].map((v) => [v, String(v)]),
    cls: "xs mono",
    width: 90,
    onChange: (v) => {
      view.per = Number(v);
      view.page = 1;
      changed();
    },
  });
  return el(
    "div",
    { class: "pager" },
    el("span", { class: "row gap6" }, "Rows", select),
    el(
      "div",
      { class: "nums" },
      btn({ class: "pg", "aria-label": "Previous page", disabled: view.page <= 1 ? true : null, onclick: () => go(view.page - 1) }, icon(I.chevLeft, 14, { w: 2 })),
      nums,
      btn({ class: "pg", "aria-label": "Next page", disabled: view.page >= pages ? true : null, onclick: () => go(view.page + 1) }, icon(I.chevRight, 14, { w: 2 })),
    ),
  );
}

/** A sortable header for a CSS-grid list. */
function sortHead(label, key, view, onChange, cls) {
  const on = view.sort === key;
  return el(
    "span",
    { class: cls || null },
    btn(
      {
        class: "sortable",
        onclick: () => {
          if (view.sort === key) view.dir = view.dir === "asc" ? "desc" : "asc";
          else {
            view.sort = key;
            view.dir = "asc";
          }
          view.page = 1;
          onChange();
        },
      },
      label,
      el("span", { class: "ar", text: on ? (view.dir === "asc" ? "↑" : "↓") : "" }),
    ),
  );
}

// -------------------------------------------------------------------- loader

/* The boot animation and the per-page loader, after the design, driven by real
 * loading: each is shown while a screen's first fetches are pending and
 * removed the frame they answer. A page whose data is already cached shows
 * neither. With reduced motion the tiles do not move. */
const LOADER_TIPS = [
  "Press `/` anywhere in the portal to jump straight to search.",
  "Every agent uses one endpoint. Connect a client once and it can reach all your stores.",
  "Watched sources re-index themselves when files change, so manual re-index runs are rarely needed.",
  "Files that look like credentials stay out of the index until you decide on them in Review.",
  "Your `.gitignore` rules are respected, so build output and vendored code never get embedded.",
  "Everything Semlith keeps lives under `~/.semlith`. Your code is indexed and searched on this machine.",
  "Keep code and docs in separate stores when you want sharper, less noisy results.",
  "Use “Search it” on a store’s page to search that store alone.",
  "Session replay in the Ledger shows what an agent did after each answer.",
  "Fewer tokens compares what agents were sent with reading every matching file in full.",
  "Runs queue behind each other up to the limit in Settings, and each one lives in the daemon.",
  "A long run can be paused and resumed later without losing progress.",
  "Prefer the terminal? `semlith index ~/path` does what the new-store wizard does.",
  "Undecided files in Review stay out. Nothing is embedded until you have decided on it.",
  "Agents › Health lists every client found on this machine and whether it can reach Semlith.",
];

function nextTip() {
  let i = 0;
  try {
    i = (parseInt(localStorage.getItem("semlith-tip") || "-1", 10) + 1) % LOADER_TIPS.length;
    localStorage.setItem("semlith-tip", String(i));
  } catch (_) {
    i = Math.floor(Math.random() * LOADER_TIPS.length);
  }
  return LOADER_TIPS[(i * 7) % LOADER_TIPS.length];
}

const TILE_MAP = "LOMMLOMOOMDDMDOX";
const TILE_INK = { L: "#85A8B8", O: "#EFA53C", M: "#6E93A6", D: "#446980", X: "#2F4B60" };

/** Open a loader. `full` is the boot screen; otherwise it covers `area`.
 * Returns a function that removes it. */
function openLoader(full, area) {
  const reduce = stillness();
  const vmin = Math.min(window.innerWidth, window.innerHeight);
  const S = Math.round(full ? Math.min(156, vmin * 0.32) : Math.min(76, vmin * 0.2));
  const t = S * 0.2134;
  const p = S * 0.2622;
  const grey = isDark() ? "#3E4E5B" : "#B3C0CA";
  const stage = el("div", { class: "stage" });
  stage.style.width = `${S}px`;
  stage.style.height = `${S}px`;
  const tiles = TILE_MAP.split("").map((k, i) => {
    const r = Math.floor(i / 4);
    const c = i % 4;
    const d = el("div", { class: "tile" });
    Object.assign(d.style, {
      left: `${(c * p).toFixed(2)}px`,
      top: `${(r * p).toFixed(2)}px`,
      width: `${t.toFixed(2)}px`,
      height: `${t.toFixed(2)}px`,
      borderRadius: `${(t * 0.19).toFixed(2)}px`,
      background: TILE_INK[k],
    });
    stage.append(d);
    return { d, r, c, k };
  });
  const cap = el("div", { class: "cap" });
  const status = el("div", { class: "status", text: "collecting chunks" });
  if (full) cap.append(el("div", { class: "word", text: "Semlith" }), status);
  else {
    const txt = el("div", { class: "tiptxt" });
    nextTip()
      .split("`")
      .forEach((part, i) => part && txt.append(i % 2 ? el("code", { text: part }) : document.createTextNode(part)));
    cap.append(el("div", { class: "tipbox" }, el("div", { class: "tiplab", text: "Tip" }), txt));
  }
  const ov = el("div", { class: full ? "ld" : "ld area", role: "status", "aria-label": "Loading Semlith" }, stage, cap);
  if (!full && area) {
    const b = area.getBoundingClientRect();
    if (b.width > 0 && b.height > 0) Object.assign(ov.style, { inset: "auto", left: `${b.left}px`, top: `${b.top}px`, width: `${b.width}px`, height: `${b.height}px` });
  }
  document.body.append(ov);
  const timers = [];
  const at = (ms, fn) => timers.push(setTimeout(fn, ms));
  let alive = true;
  if (!reduce && ov.animate) {
    if (full) {
      const cluster = { L: 0, M: 1, D: 2, X: 2, O: 3 };
      const R = vmin * 0.46;
      tiles.forEach((x, rank) => {
        const delay = rank * 40 + cluster[x.k] * 90;
        const a = Math.random() * Math.PI * 2;
        const dist = R * (0.55 + Math.random() * 0.45);
        const dx = Math.cos(a) * dist;
        const dy = Math.sin(a) * dist;
        const rot = (Math.random() - 0.5) * 200;
        x.d.animate(
          [
            { transform: `translate(${dx}px,${dy}px) rotate(${rot}deg) scale(.5)`, opacity: 0 },
            { offset: 0.26, transform: `translate(${dx * 0.93}px,${dy * 0.93}px) rotate(${rot * 0.85}deg) scale(.58)`, opacity: 1, easing: "cubic-bezier(.2,.9,.25,1.14)" },
            { transform: "none", opacity: 1 },
          ],
          { duration: 1000, delay, fill: "both" },
        );
        x.d.animate([{ backgroundColor: grey }, { backgroundColor: grey, offset: 0.7 }, { backgroundColor: TILE_INK[x.k] }], { duration: 1000, delay, fill: "both" });
      });
      at(520, () => alive && (status.textContent = "clustering by meaning"));
      at(1700, () => alive && (status.textContent = "linked · ready"));
    }
    const shuffle = () => {
      const perm = tiles.map((_, i) => i).sort(() => Math.random() - 0.5);
      tiles.forEach((x, i) => {
        const q = perm[i];
        const dx = ((q % 4) - x.c) * p;
        const dy = (Math.floor(q / 4) - x.r) * p;
        x.d.animate([{ transform: `translate(${dx}px,${dy}px) scale(.45)`, opacity: 0.4 }, { transform: "none", opacity: 1 }], {
          duration: 620,
          delay: (x.r + x.c) * 22,
          easing: "cubic-bezier(.5,0,.1,1.25)",
          fill: "backwards",
        });
      });
    };
    const wave = () =>
      tiles.forEach((x) =>
        x.d.animate([{ transform: "none" }, { offset: 0.4, transform: "translateY(-12%) scale(1.08)", filter: "brightness(1.12)" }, { transform: "none" }], {
          duration: 480,
          delay: (x.r + x.c) * 60,
          easing: "ease-in-out",
        }),
      );
    // Loops for as long as the wait does: the loader reports a wait, it is
    // not a fixed-length show.
    const loop = (start) => {
      if (!alive) return;
      shuffle();
      at(820, () => alive && wave());
      at(1450, () => loop(false));
      return start;
    };
    at(full ? 1800 : 0, () => loop(true));
  }
  return () => {
    if (!alive) return;
    alive = false;
    timers.forEach(clearTimeout);
    ov.remove();
  };
}

// --------------------------------------------------------------------- state

const state = {
  screen: "app",
  route: { page: "home", parts: [] },
  theme: "system",
  tier: "l",
  navOpen: true,
  home: "",
  /** The wizard's working state, while it is open. */
  wz: null,
  /** Carried into Search and Graph from another page. */
  pending: {},
  /** This session's searches, newest first. */
  recents: [],
  /** The run a stop dialog or a button just changed, painted before the poll. */
  dismissed: new Set(),
  version: "",
};

const store = (name) => (data.stores?.stores || []).find((s) => s.name === name);
const liveStores = () => (data.stores?.stores || []).filter((s) => !s.missing && !s.unopened);
// Semlith Cloud stores this machine reads and never writes: listed beside the
// local ones with their badge, never among them, since nothing that totals,
// opens or writes a store applies to one.
const remoteStores = () => data.stores?.remote || [];

// ---------------------------------------------------------------- data cache

/* Every route the portal reads, by name. A page lists the names it needs; the
 * router shows it at once when they are all in hand (and refreshes them behind
 * it), and shows the page loader only while one of them is still on its way. */
const SOURCES = {
  stores: "/api/stores",
  about: "/api/about",
  agents: "/api/agents",
  ledger: "/api/ledger",
  privacy: "/api/privacy",
  runs: "/api/index/runs",
  refused: "/api/refused",
  decisions: "/api/refused?decisions=1",
  detail: "/api/stores?detail=1",
  corpus: "/api/corpus",
  accel: "/api/accel",
  schedules: "/api/schedules",
  prices: "/api/prices",
  replay: "/api/ledger/replay",
  languages: "/api/languages",
  cloud: "/api/cloud",
  graphpeek: () => `/api/graph?${new URLSearchParams({ store: (graphPeekFor = graphStore()), limit: "12" })}`,
  graphmap: () => `/api/map?${new URLSearchParams({ store: (graphMapFor = graphStore()), shown: "12" })}`,
  coverage: "/api/stores?coverage=1",
};

const data = {};
const loading = {};
const loadedAt = {};

function load(key, force) {
  if (!force && data[key] !== undefined) return Promise.resolve(data[key]);
  if (loading[key]) return loading[key];
  // A source may depend on the page (the graph's store): then it is a function.
  const p = api(typeof SOURCES[key] === "function" ? SOURCES[key]() : SOURCES[key])
    .then((value) => {
      data[key] = value;
      loadedAt[key] = Date.now();
      if (key === "stores" || key === "coverage") noteStores(value);
      if (key === "runs") holdPausing(value), noteRuns();
      if (key === "about") noteAbout(value);
      return value;
    })
    .finally(() => {
      delete loading[key];
    });
  loading[key] = p;
  return p;
}

// Runs a Pause was just pressed on, until when. A poll that left before the
// click answers "running" after it; for a few seconds that is read as the
// "pausing" the click already showed, until the daemon says otherwise.
const PAUSING = new Map();
function holdPausing(value) {
  const now = Date.now();
  for (const r of (value && value.runs) || []) {
    const key = `${r.store}:${r.id}`;
    const until = PAUSING.get(key);
    if (!until) continue;
    if (r.status !== "running" || now > until) PAUSING.delete(key);
    else r.status = "pausing";
  }
}

function loadMany(keys, force) {
  return Promise.all(keys.map((k) => load(k, force).catch((e) => ({ __error: e }))));
}

const cached = (keys) => keys.every((k) => data[k] !== undefined);

/** Refetch and repaint whatever on screen reads these. */
async function refresh(keys) {
  const list = Array.isArray(keys) ? keys : [keys];
  await loadMany(list, true);
  // A store that appears while the first-run screen is up — made from the
  // terminal, say — ends the first run.
  if (state.screen === "welcome" && state.route.page !== "welcome" && (data.stores?.stores || []).length) return render();
  paintChrome();
  if (current.onData) current.onData(list);
  else if (current.live && list.some((k) => current.live.includes(k))) {
    // A view that names what it draws from a live domain is redrawn only when
    // that part moved, so a run's progress does not rebuild a settings form.
    if (current.view && current.view.sig) {
      const sig = current.view.sig(current.route);
      if (sig === current.sig) return;
      current.sig = sig;
    }
    repaint();
  }
}

function noteStores(value) {
  if (value === data.coverage) data.stores = value;
  const first = liveStores()[0];
  if (!state.home && first && first.dir) {
    const m = first.dir.match(/^(\/Users\/[^/]+|\/home\/[^/]+|[A-Z]:\\Users\\[^\\]+)/);
    if (m) state.home = m[1];
  }
}

function noteAbout(about) {
  state.version = about.version || "";
  if (about.store_home && !state.home) {
    const m = String(about.store_home).match(/^(.*)[\\/]\.semlith/);
    if (m) state.home = m[1];
  }
}

// --------------------------------------------------------------------- live

/* One clock for the whole page: six change counters, polled once a second;
 * only the routes behind the counters that moved are refetched. A hidden tab
 * asks nothing. No view starts a timer of its own. */
const live = { seen: {}, timer: null };

const DOMAIN_KEYS = {
  stores: ["stores", "refused", "decisions"],
  runs: ["runs"],
  clients: ["agents"],
  ledger: ["ledger"],
  events: ["stores"],
  privacy: ["privacy", "refused", "decisions"],
};

async function pollChanges() {
  if (document.visibilityState === "hidden") return;
  let counters;
  try {
    counters = await api("/api/changes");
  } catch (_) {
    return;
  }
  const moved = [];
  for (const [domain, value] of Object.entries(counters)) {
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
  const keys = new Set();
  for (const d of moved) for (const k of DOMAIN_KEYS[d] || []) if (data[k] !== undefined || k === "stores" || k === "runs") keys.add(k);
  // The figures that are expensive to read are dropped rather than refetched:
  // the next page that wants them reads them again.
  if (moved.includes("stores")) {
    delete data.corpus;
    delete data.coverage;
    if (state.route.page === "store") keys.add("detail");
    else delete data.detail;
  }
  if (keys.size) refresh([...keys]);
}

function startLive() {
  if (live.timer) return;
  live.timer = setInterval(pollChanges, 1000);
  document.addEventListener("visibilitychange", () => document.visibilityState === "visible" && pollChanges());
}

/** The runs that are not finished, per store. */
const LIVE_RUN = new Set(["queued", "scanning", "review", "running", "pausing", "paused", "held", "stopping"]);

function runsOf(name) {
  return (data.runs?.runs || []).filter((r) => r.store === name);
}

function activeRun(name) {
  const all = (data.runs?.runs || []).filter((r) => LIVE_RUN.has(r.status) && (!name || r.store === name));
  return all.find((r) => r.status === "running") || all[0] || null;
}

// How far a run is, over all of it. Reading finishes long before embedding
// does, so a bar on bytes read reached 100 % and then sat there (or went back
// to 0 when embedding began). The daemon's progress is the share read times
// the share of what was written that is embedded; the bar never moves
// backwards within a run and never reaches 100 % before the run is done.
// pending_share alone (rc.3) ignored the files not read yet, so a run whose
// embedding kept up sat at 99 % from its first second.
const RUN_HIGH = new Map();
function runPct(run) {
  if (!run) return 0;
  if (run.status === "done") return 100;
  let p;
  if (typeof run.progress === "number") p = run.progress * 100;
  else if (typeof run.pending_share === "number") p = (1 - run.pending_share) * 100;
  else if (run.bytes_total) p = (run.bytes / run.bytes_total) * 50;
  else p = run.total ? (run.scanned / run.total) * 50 : 0;
  const key = `${run.store}:${run.id}:${run.started_at || run.submitted || ""}`;
  const high = Math.max(RUN_HIGH.get(key) || 0, Math.min(99, Math.max(0, p)));
  RUN_HIGH.set(key, high);
  return high;
}

// A daemon that keeps one account of its runs (run-truth spec, rc.4) sends
// `phases` and `expected_chunks`; its `eta_ms` covers the whole run.
const truthRun = (run) => run.phases !== undefined || run.expected_chunks !== undefined;

// Time left. From a run-truth daemon, its own estimate, never extrapolated
// here. From an older one, the share still to do over the time spent so far,
// which holds through embedding where its estimate covers reading only.
function runLeftMs(run) {
  if (!run || run.status !== "running") return null;
  if (truthRun(run)) return run.eta_ms ?? null;
  const done = runPct(run) / 100;
  if (done < 0.05 || !run.elapsed_ms) return run.eta_ms ?? null;
  return Math.round((run.elapsed_ms - (run.queued_ms || 0)) * (1 - done) / done);
}

/** "about 12 min", the way an estimate is said: never to the second. */
function spellAbout(ms) {
  if (ms === null || ms === undefined) return "estimating…";
  const seconds = Math.round(ms / 1000);
  if (seconds < 10) return "almost done";
  if (seconds < 60) return "under a minute";
  const minutes = Math.round(seconds / 60);
  if (minutes < 90) return `about ${minutes} min`;
  return `about ${Math.floor(minutes / 60)} h ${String(minutes % 60).padStart(2, "0")} min`;
}

/** A running run's time left, in the words its card uses. */
function runLeftText(run) {
  return truthRun(run) ? spellAbout(runLeftMs(run)) : spellLeft(runLeftMs(run));
}

/** The low/high spread of a run's estimate, for a tooltip. */
function runRangeTip(run) {
  const range = run.eta_range_ms;
  return Array.isArray(range) && range.length === 2 ? `Between ${spellTook(range[0])} and ${spellTook(range[1])}, from the spread of the last minute's rate` : null;
}

// What each phase of a run is called on screen (spec §3.2).
const PHASE_LABEL = {
  decisions: "Applying decisions",
  queued: "Queued",
  walk: "Finding files",
  credentials: "Checking for credentials",
  rules: "Matching rules",
  read: "Reading and chunking",
  lane: "Starting a lane",
  embed: "Embedding",
  images: "Embedding images",
  graph: "Writing the graph",
  drain: "Finishing embeddings in flight",
  save: "Writing the index to disk",
  checkpoint: "Checkpoint",
  finalize: "Finishing up",
  undo: "Undoing",
};

function phaseLabel(phase, detail, lane) {
  if (phase !== "lane") return PHASE_LABEL[phase] || phase;
  const named = lane ? laneName(lane) : (/^(.+?)\s+(?:is\s+)?(?:loading|compiling|ready|failed|waiting|downloading|starting)/i.exec(detail || "") || [])[1];
  if (!named) return PHASE_LABEL.lane;
  return `Starting ${/^the /i.test(named) ? named : `the ${named}`}`;
}

/** What a run is doing now, in one line: the phase and its sentence. An
 * older daemon sends a sentence in `phase` itself. */
function phaseText(run) {
  if (!run.phase) return "";
  if (!PHASE_LABEL[run.phase]) return run.phase;
  return [phaseLabel(run.phase, run.phase_detail, run.lane), run.phase_detail].filter(Boolean).join(" — ");
}

// A daemon time in milliseconds: phases carry ms, older fields seconds.
const msOf = (t) => (t > 1e12 ? t : t * 1000);

/* When this page saw a run wait in the queue, per run: the snapshot says a run
 * is queued, not since when or for how long, and a run held for review was
 * submitted when its scan began. */
const QUEUED = new Map();

/** A run's steps from its own events: the queue, then every phase it sent. */
function runSteps(r) {
  const steps = [];
  const queued = r.status === "queued";
  const key = `${r.store}:${r.id}`;
  let q = QUEUED.get(key);
  if (queued && !q) QUEUED.set(key, (q = { at: Date.now(), until: null }));
  if (q && !queued && q.until == null) q.until = Date.now();
  if (q) steps.push({ label: "Queued", at: q.at, until: q.until, detail: queued ? (r.position > 1 ? `${plural(r.position - 1, "run")} ahead of this one` : "next in line") : "" });
  if (queued) return steps;
  const phases = r.phases || [];
  phases.forEach((p, i) => {
    const current = i === phases.length - 1 && p.until == null;
    steps.push({
      label: phaseLabel(p.phase, p.detail, p.lane),
      at: msOf(p.at),
      until: p.until != null ? msOf(p.until) : null,
      detail: (current && r.phase === p.phase && r.phase_detail) || p.detail || (p.count != null ? n(p.count) : ""),
    });
  });
  // An older daemon names no phases: one step, in its own words.
  if (!r.phases && LIVE_RUN.has(r.status)) steps.push({ label: "Indexing", at: msOf(r.started_at || r.submitted || 0), until: null, detail: r.phase || "" });
  return steps;
}

/* The minimum dwell (spec §3.2, W5). A phase that came and went in a blink is
 * still drawn for DWELL_MS, queued behind the real one; the display never runs
 * ahead of the events and never more than LAG_MS behind them — past that it
 * skips to catch up. Kept per run here, so a repaint never resets the queue. */
const DWELL_MS = 700;
const LAG_MS = 3000;
const SHOWN = new Map();
function dwell(key, steps, now) {
  const at = now || Date.now();
  if (!steps.length) return { steps, behind: false };
  let s = SHOWN.get(key);
  if (!s) SHOWN.set(key, (s = { i: 0, since: at }));
  const last = steps.length - 1;
  s.i = Math.min(s.i, last);
  while (s.i < last && at - steps[s.i + 1].at > LAG_MS) {
    s.i += 1;
    s.since = at;
  }
  if (s.i < last && at - s.since >= DWELL_MS) {
    s.i += 1;
    s.since = at;
  }
  return { steps: steps.slice(0, s.i + 1), behind: s.i < last };
}

/** Steps as a timeline: label, when it began, how long it took, its counters;
 * the last one pulses while the run is live. */
function timeline(steps, live) {
  const now = Date.now();
  return el(
    "ol",
    { class: "tl" },
    steps.map((s, i) => {
      const cur = live && i === steps.length - 1;
      const took = s.until != null ? s.until - s.at : now - s.at;
      return el(
        "li",
        { class: `tl-step${cur ? " cur" : ""}` },
        el("span", { class: `dot ${cur ? "blue pulse" : "green"}` }),
        el(
          "div",
          { class: "tl-body" },
          el("div", { class: "tl-head" }, el("span", { class: "tl-label", text: s.label }), el("span", { class: "tl-when", text: `${s.at ? new Date(s.at).toTimeString().slice(0, 8) : ""} · ${spellTook(Math.max(0, took))}` })),
          s.detail ? el("div", { class: "tl-detail", text: s.detail }) : null,
        ),
      );
    }),
  );
}

let lastRunState = {};
function noteRuns() {
  // A run that finished while the page was open says so once, wherever the
  // reader is.
  const now = {};
  for (const r of data.runs?.runs || []) now[r.id] = r.status;
  for (const [id, before] of Object.entries(lastRunState)) {
    const after = now[id];
    if (LIVE_RUN.has(before) && after === "done") {
      const run = (data.runs.runs || []).find((x) => String(x.id) === id);
      if (run && !(state.screen === "wizard" && state.wz && state.wz.store === run.store)) toast(`${run.store} is indexed and ready`);
    }
  }
  lastRunState = now;
  for (const fn of [...runsListeners]) {
    try {
      fn();
    } catch (_) {
      /* one listener failing must not stop the rest */
    }
  }
}

/* Anything that wants to hear when the runs answer changes: the wizard, a
 * store's Runs tab. Each returns its own way off. */
const runsListeners = new Set();
function onRunsChange(fn) {
  runsListeners.add(fn);
  return () => runsListeners.delete(fn);
}

// ------------------------------------------------------------------- theme

function isDark() {
  if (state.theme === "dark") return true;
  if (state.theme === "light") return false;
  return window.matchMedia("(prefers-color-scheme: dark)").matches;
}

function applyTheme() {
  // The attribute is always written, as `light` or `dark`: "system" follows
  // `prefers-color-scheme` live through the listener below.
  document.documentElement.setAttribute("data-theme", isDark() ? "dark" : "light");
  for (const img of document.querySelectorAll("img.logo24")) img.src = isDark() ? "logo-dark.svg" : "logo.svg";
  for (const g of document.querySelectorAll(".theme")) {
    for (const b of g.children) b.setAttribute("aria-pressed", String(b.dataset.value === state.theme));
  }
}

function setTheme(next) {
  state.theme = next;
  try {
    localStorage.setItem("semlith-theme", next);
  } catch (_) {
    /* private window */
  }
  applyTheme();
}

function themeControl() {
  return el(
    "div",
    { class: "theme", role: "group", "aria-label": "Theme" },
    [
      ["light", "Light", I.sun],
      ["dark", "Dark", I.moon],
      ["system", "System", I.system],
    ].map(([value, label, d]) =>
      btn(
        {
          "data-value": value,
          "aria-pressed": String(state.theme === value),
          "aria-label": `${label} theme`,
          "data-tip": `${label}${value === "system" ? " · follows this machine" : ""}`,
          onclick: () => setTheme(value),
        },
        icon(d, 14),
      ),
    ),
  );
}

function logo() {
  return el("img", { class: "logo24", src: isDark() ? "logo-dark.svg" : "logo.svg", alt: "Semlith", width: 24, height: 24 });
}

// -------------------------------------------------------------------- router

const PAGES = {
  home: { title: "Home", group: "Workspace" },
  stores: { title: "Stores", group: "Workspace" },
  search: { title: "Search", group: "Workspace" },
  graph: { title: "Graph", group: "Workspace" },
  agents: { title: "Agents", group: "Agents" },
  ledger: { title: "Ledger", group: "Agents" },
  reports: { title: "Reports", group: "Agents" },
  privacy: { title: "Privacy", group: "Machine" },
  settings: { title: "Settings", group: "Machine" },
};
const NAV = [
  ["Workspace", ["home", "stores", "search", "graph"]],
  ["Agents", ["agents", "ledger", "reports"]],
  ["Machine", ["privacy", "settings"]],
];

function parseHash() {
  const raw = (location.hash || "").replace(/^#\/?/, "");
  const parts = raw.split("/").filter(Boolean).map(decodeURIComponent);
  const page = parts.shift() || "home";
  return { page, parts };
}

/** Navigate. `go("store", name, "runs")` writes `#/store/<name>/runs`. */
function go(page, ...parts) {
  const hash = `#/${[page, ...parts.filter((p) => p !== undefined && p !== null && p !== "")].map(encodeURIComponent).join("/")}`;
  if (state.tier === "s" && state.navOpen) setNav(false);
  if (location.hash === hash) render();
  else location.hash = hash;
}

/** The view on screen: its node, its live keys, and its in-place painter. */
let current = { node: null, live: [], onData: null };
let renderGen = 0;
let pageLoader = null;

const VIEWS = {};

async function render() {
  const route = parseHash();
  state.route = route;
  const root = document.getElementById("root");
  const stores = liveStores();
  const hasAny = (data.stores?.stores || []).length > 0;

  if (route.page === "new") {
    state.screen = "wizard";
    current = { node: null, live: [], onData: null };
    shell.main = null;
    const node = wizardScreen(route.parts);
    fill(root, node);
    return;
  }
  if (route.page === "welcome" || (!hasAny && !["store", "settings", "privacy", "agents"].includes(route.page))) {
    state.screen = "welcome";
    shell.main = null;
    current = { node: null, live: [], onData: null };
    fill(root, welcomeScreen());
    return;
  }
  state.screen = "app";
  if (!shell.main || !root.contains(shell.main)) {
    fill(root, buildShell());
    applyTier(true);
  }
  const view = VIEWS[route.page] || VIEWS.home;
  const pageId = VIEWS[route.page] ? route.page : "home";
  const navId = pageId === "store" ? "stores" : pageId;
  for (const node of document.querySelectorAll("[data-nav]")) {
    if (node.getAttribute("data-nav") === navId) node.setAttribute("aria-current", "page");
    else node.removeAttribute("aria-current");
  }
  paintCrumb();
  document.title = `Semlith · ${pageId === "store" ? route.parts[0] || "Store" : PAGES[pageId].title}`;

  const mine = ++renderGen;
  const needs = view.needs ? view.needs(route) : [];
  if (pageLoader) {
    pageLoader();
    pageLoader = null;
  }
  shell.main.className = view.fill ? "main fill" : "main";
  if (!cached(needs)) {
    fill(shell.main);
    pageLoader = openLoader(false, shell.main);
    await loadMany(needs);
    if (pageLoader) {
      pageLoader();
      pageLoader = null;
    }
    if (mine !== renderGen) return;
  } else {
    // Shown at once from the cache, and brought up to date behind it.
    const stale = needs.filter((k) => Date.now() - (loadedAt[k] || 0) > 4000);
    if (stale.length) loadMany(stale, true).then(() => mine === renderGen && (current.onData ? current.onData(stale) : repaint()));
  }
  if (mine !== renderGen) return;
  mount(view, route);
}

function mount(view, route) {
  let node;
  const holder = { live: view.live || [], onData: null };
  try {
    node = view.render(route, holder);
  } catch (e) {
    console.error(e);
    node = el("div", { class: "page" }, errorBox(`This page failed to draw: ${e.message}`));
  }
  current = { node, live: holder.live, onData: holder.onData, view, route, sig: view.sig ? view.sig(route) : null };
  fill(shell.main, node);
  paintChrome();
}

/* Draw the current view again from the cache, keeping the scroll position and
 * the focused field. Inputs carry `data-keep` so the new copy can be found. */
function repaint() {
  if (!shell.main || !current.view) return;
  const at = shell.main.scrollTop;
  const inner = [...shell.main.querySelectorAll("[data-scroll-keep]")].map((n) => [n.getAttribute("data-scroll-keep"), n.scrollTop]);
  const focus = document.activeElement;
  const keep = focus && focus.getAttribute ? focus.getAttribute("data-keep") : null;
  const sel = keep && "selectionStart" in focus ? [focus.selectionStart, focus.selectionEnd] : null;
  // A view that draws synchronously is patched in place, so nothing under the
  // reader is rebuilt; the rest are drawn again.
  if (current.view.morph && current.view.morph(current.route) && shell.main.firstChild) {
    const holder = { live: current.view.live || [], onData: null };
    let node;
    try {
      node = current.view.render(current.route, holder);
    } catch (e) {
      console.error(e);
      return mount(current.view, current.route);
    }
    morph(shell.main.firstChild, node);
    current = { ...current, live: holder.live, onData: holder.onData, sig: current.view.sig ? current.view.sig(current.route) : null };
    paintChrome();
    return;
  }
  mount(current.view, current.route);
  shell.main.scrollTop = at;
  for (const [k, top] of inner) {
    const n = shell.main.querySelector(`[data-scroll-keep="${k}"]`);
    if (n) n.scrollTop = top;
  }
  if (keep) {
    const again = shell.main.querySelector(`[data-keep="${keep}"]`);
    if (again) {
      again.focus();
      if (sel && again.setSelectionRange) {
        try {
          again.setSelectionRange(sel[0], sel[1]);
        } catch (_) {
          /* not a text field */
        }
      }
    }
  }
}

// --------------------------------------------------------------------- shell

const shell = {};

function buildShell() {
  const navToggle = btn({ class: "btn icon32", "aria-label": "Toggle menu", "data-tip": "Toggle menu", onclick: () => setNav(!state.navOpen) }, icon(I.menu, 15, { w: 1.8 }));
  const crumb = el("div", { class: "crumb" });
  const runPill = btn({ class: "run-pill hide-xs", hidden: true, onclick: () => runPill.dataset.store && go("store", runPill.dataset.store, "runs") });
  const top = el(
    "header",
    { class: "top" },
    navToggle,
    btn({ class: "brand", "aria-label": "Semlith — go to Home", onclick: () => go("home") }, logo(), el("span", { class: "word hide-sm", text: "Semlith" })),
    el("span", { class: "vrule hide-sm" }),
    crumb,
    runPill,
    el("span", { class: "spacer" }),
    btn(
      { class: "ask", "aria-label": "Ask the index a question", onclick: () => go("search") },
      icon(I.searchSm, 14, { w: 1.8 }),
      el("span", { class: "t hide-sm", text: "Ask the index a question" }),
      el("span", { class: "kbd hide-sm", text: "/" }),
    ),
    btn({ class: "btn primary", onclick: () => openWizard() }, icon(I.plus, 14, { w: 2.2 }), el("span", { class: "hide-sm", text: "New store" })),
    el("div", { class: "hide-xs" }, themeControl()),
  );
  const navGroups = NAV.map(([label, ids]) =>
    el(
      "div",
      { class: "nav-group" },
      el("div", { class: "nav-label", text: label }),
      ids.map((id) =>
        btn(
          { class: "nav-item", "data-nav": id, onclick: () => go(id) },
          icon(I[id], 15, { w: 1.6 }),
          el("span", { class: "lab", text: PAGES[id].title }),
          el("span", { class: "nav-badge", "data-badge": id, hidden: true }),
        ),
      ),
    ),
  );
  const daemonFacts = el("div", { class: "facts" });
  const nav = el(
    "nav",
    { class: "nav", "aria-label": "Sections" },
    navGroups,
    el("div", { class: "spacer" }),
    el("div", { class: "daemon" }, el("div", { class: "who" }, dot("green"), "daemon running"), daemonFacts),
  );
  nav.querySelector(".daemon .dot").classList.add("slow");
  const rail = el(
    "nav",
    { class: "rail", "aria-label": "Sections" },
    NAV.map(([, ids]) =>
      el(
        "div",
        { class: "rail-group" },
        ids.map((id) =>
          btn(
            { class: "rail-item", "data-nav": id, "aria-label": PAGES[id].title, "data-tip": PAGES[id].title, onclick: () => go(id) },
            icon(I[id], 17, { w: 1.6 }),
            el("span", { class: "rb", "data-rail-badge": id, hidden: true }),
          ),
        ),
      ),
    ),
    el("div", { class: "spacer" }),
    el("span", { class: "rail-daemon", "data-tip": "Daemon running", "data-tip-color": "var(--green)" }, el("span", { class: "dot green d7 slow" })),
  );
  const scrim = el("div", { class: "nav-scrim", hidden: true, onclick: () => setNav(false) });
  const down = el("div", { class: "daemon-down", hidden: true, role: "alert" }, icon(I.alert, 15), el("span", { text: "The daemon is not answering. Start it again with `semlith start`; this page reconnects by itself." }));
  const main = el("main", { class: "main", id: "main" });
  const row = el("div", { class: "shell-row" }, nav, scrim, rail, main);
  Object.assign(shell, { top, crumb, runPill, nav, rail, scrim, main, daemonFacts, down, navToggle });
  const app = el("div", { id: "app" }, top, down, row);
  // The width tier is the app's, not the window's, so a split-screen browser
  // gets the phone layout when it is phone-sized.
  const measure = () => applyTier(false);
  try {
    new ResizeObserver(measure).observe(app);
  } catch (_) {
    window.addEventListener("resize", measure);
  }
  return app;
}

function applyTier(first) {
  const app = document.getElementById("app");
  if (!app) return;
  const w = app.clientWidth || window.innerWidth;
  const tier = w < 760 ? "s" : w < 1100 ? "m" : "l";
  if (tier !== state.tier || first) {
    state.tier = tier;
    state.navOpen = tier === "l";
  }
  setNav(state.navOpen);
}

function setNav(open) {
  state.navOpen = open;
  if (!shell.nav) return;
  shell.nav.hidden = !open;
  shell.rail.hidden = open || state.tier === "s";
  shell.scrim.hidden = !(open && state.tier === "s");
  shell.navToggle.setAttribute("aria-expanded", String(open));
}

function paintCrumb() {
  if (!shell.crumb) return;
  const { page, parts } = state.route;
  const isStore = page === "store";
  const title = isStore ? "Stores" : (PAGES[page] || PAGES.home).title;
  fill(
    shell.crumb,
    btn({ class: "page-link", onclick: () => go(isStore ? "stores" : page) }, title),
    isStore && parts[0] ? [el("span", { class: "sep", text: "/" }), el("span", { class: "leaf", text: parts[0] })] : null,
  );
}

function noteDaemon(up) {
  if (daemonDown === !up) return;
  daemonDown = !up;
  if (shell.down) shell.down.hidden = up;
}

/** Everything on the shell that follows data: badges, the run pill, the card. */
function paintChrome() {
  if (!shell.nav) return;
  const reviewN = (data.refused?.stores || []).reduce((a, s) => a + (s.review || 0), 0);
  const noAgent = data.agents ? !registeredClients().length : false;
  const badges = { stores: reviewN ? String(reviewN) : "", agents: noAgent ? "!" : "" };
  for (const node of document.querySelectorAll("[data-badge]")) {
    const b = badges[node.getAttribute("data-badge")] || "";
    node.hidden = !b;
    node.className = "nav-badge amber";
    setText(node, b);
  }
  for (const node of document.querySelectorAll("[data-rail-badge]")) {
    const id = node.getAttribute("data-rail-badge");
    node.hidden = !badges[id];
    const host = node.parentElement;
    host.setAttribute(
      "data-tip-rows",
      id === "stores" && badges.stores ? `to review::${badges.stores} files` : id === "agents" && badges.agents ? "status::no agent registered yet" : "",
    );
  }
  const run = activeRun();
  const pillNode = shell.runPill;
  if (run) {
    const p = Math.floor(runPct(run));
    const word = { paused: "Paused", pausing: "Pausing", queued: "Queued", review: "Waiting for review", stopping: "Stopping", held: "Held" }[run.status] || (run.kind === "compact" ? "Compacting" : "Indexing");
    pillNode.hidden = false;
    pillNode.dataset.store = run.store;
    pillNode.className = `run-pill hide-xs${run.status === "review" || run.status === "paused" ? " amber" : ""}`;
    fill(pillNode, dot(run.status === "review" || run.status === "paused" ? "amber" : "green", run.status === "running"), `${word} ${run.store}${run.status === "review" ? "" : ` · ${p}%`}`, bar(p, "w46"));
    pillNode.setAttribute("data-tip", "Open this store's runs");
  } else pillNode.hidden = true;
  const stores = liveStores().length;
  const agents = connectedCount();
  const rec = recordingWord();
  const cloudOrgs = (data.about?.cloud?.orgs || []).map((o) => o.org);
  fill(shell.daemonFacts, `${location.host} · sole writer`, el("br"), `${plural(stores, "store")} · ${agents ? `${plural(agents, "agent")} connected` : `${plural(registeredClients().length, "agent")} registered`} · ledger ${rec}${cloudOrgs.length ? ` · cloud: ${cloudOrgs.join(", ")}` : ""}`);
  const railDaemon = shell.rail.querySelector(".rail-daemon");
  if (railDaemon) railDaemon.setAttribute("data-tip-rows", rows([["address", location.host], ["role", "sole writer"], ["stores", String(stores)], ["agents", String(agents)], ["ledger", rec]]));
}

function connectedCount() {
  return (data.agents?.connections || []).length;
}

function registeredClients() {
  return (data.agents?.doctor || []).filter((c) => c.registered);
}

/** The ledger's state, in the word the daemon card uses. */
function recordingWord() {
  const r = data.ledger?.recording ?? data.about?.recording ?? data.about?.ledger;
  if (r && typeof r === "object") return r.on ? "on" : r.reason === "paused" ? "paused" : "off";
  if (r === undefined || r === null) return "on";
  return r ? "on" : "off";
}

function recordingOn() {
  return recordingWord() === "on";
}

// ------------------------------------------------------------------- welcome

/* First run: nothing indexed yet. The checks on the right are read from the
 * daemon — its address, the accelerator lanes, the machine's memory, whether
 * the embedding model is already on disk, and which agent clients are here. */
function welcomeScreen() {
  const checks = el("div", { class: "col" });
  const checkLine = meta("checking…");
  const steps = [
    ["1", "Name a store", "One index, one model, as many as you like", "10 s"],
    ["2", "Add folders or drop files", "Read where they sit — nothing is copied", "30 s"],
    ["3", "Review what looks sensitive", "Credentials are never indexed; you decide the grey zone", "1 min"],
    ["4", "Index", "In the background, on the fastest lane this machine has", "minutes"],
    ["5", "Connect your agents", "Claude Code, Cursor and the rest, in one step", "20 s"],
  ];
  const node = el(
    "div",
    { id: "app" },
    el("header", { class: "top wide-pad" }, logo(), el("span", { class: "wordmark", text: "Semlith" }), state.version ? el("span", { class: "count-chip", text: state.version }) : null, el("span", { class: "spacer" }), themeControl()),
    el(
      "div",
      { class: "welcome-body" },
      el(
        "div",
        { class: "welcome-grid" },
        el(
          "div",
          { class: "welcome-card" },
          el("span", { class: "first-run" }, el("span", { class: "dot amber" }), "FIRST RUN · NOTHING INDEXED YET"),
          el(
            "div",
            { class: "col gap6" },
            el("div", { class: "welcome-h", text: "Give your agents a memory of your code, kept on this machine." }),
            el("div", { class: "welcome-lead", text: "Semlith reads your folders once, keeps a searchable index under ~/.semlith, and answers every agent over one local endpoint. Your first store takes a few minutes." }),
          ),
          el(
            "div",
            { class: "steps5" },
            steps.map(([num, t, d, time]) =>
              el("div", { class: "s" }, el("span", { class: "num-circle", text: num }), el("div", { class: "col" }, el("span", { class: "t-m", text: t }), el("span", { class: "muted t-sm", text: d })), el("span", { class: "t-mono muted", text: time })),
            ),
          ),
          el(
            "div",
            { class: "row" },
            btn({ class: "btn primary lg", onclick: () => openWizard({ onboarding: true }) }, "Create your first store", icon(I.arrow, 15, { w: 2 })),
            btn({ class: "btn lg t-m", onclick: () => adoptFlow() }, "Adopt an existing .semlith"),
          ),
          el(
            "div",
            { class: "term-hint" },
            el("span", { class: "muted t-sm", text: "Prefer the terminal?" }),
            btn({ class: "term-copy", onclick: () => copy("semlith index ~/path/to/folder") }, "semlith index ~/path/to/folder", el("span", { class: "c", text: "Copy" })),
          ),
        ),
        el(
          "div",
          { class: "card flexcol" },
          el("div", { class: "card-h" }, el("span", { class: "card-t grow", text: "This machine" }), checkLine),
          checks,
          el("div", { class: "grow" }),
          el("div", { class: "shield-foot" }, icon(I.shield, 16), el("span", { text: "Nothing leaves this machine. The embedding model is the only download setup needs, and it asks before it makes it." })),
        ),
      ),
    ),
  );
  const row = (k, v, d, kind) =>
    el(
      "div",
      { class: "check-row" },
      el("span", { class: "ic" }, kind === "wait" ? el("span", { class: "spinner" }) : kind === "pend" ? el("span", { class: "check-pend" }) : kind === "bad" ? el("span", { class: "check-bad" }, icon(I.x, 11, { w: 3 })) : el("span", { class: "check-ok" }, icon(I.check, 11, { w: 3 }))),
      el("div", { class: "col gap2" }, el("div", { class: "row base nowrap" }, el("span", { class: "k", text: k }), el("span", { class: "v", text: v })), d ? el("span", { class: "d", text: d }) : null),
    );
  const paint = () => {
    const about = data.about;
    const accel = data.accel;
    const priv = data.privacy;
    const agents = data.agents;
    const limits = data.runs?.limits;
    const list = [];
    list.push(about ? row("Daemon", "running", `${about.bind} · loopback only, the sole writer`) : row("Daemon", "checking…", "", "wait"));
    if (accel) {
      const lanes = (accel.lanes || []).filter((l) => l.enabled);
      const best = lanes.find((l) => l.lane !== "cpu") || lanes[0];
      list.push(row("Accelerator", best ? best.label || best.lane : "CPU", best ? `${best.device || "device named when it starts"} · ${laneWord(best.status)}` : "the CPU carries the work"));
    } else list.push(row("Accelerator", "checking…", "", "wait"));
    if (limits && limits.machine) {
      const m = limits.machine;
      list.push(row("Memory", `${(m.total_memory_mb / 1024).toFixed(0)} GiB`, `${(m.available_memory_mb / 1024).toFixed(1)} GiB free · room for ${plural(limits.runs_at_once?.value || 1, "run")} at a time`));
    } else list.push(row("Memory", "checking…", "", "wait"));
    if (priv) {
      const model = (priv.downloads || [])[0];
      list.push(
        model && model.cached
          ? row("Embedding model", bytes(model.bytes), "On disk already. Nothing to download.")
          : row("Embedding model", model ? bytes(model.bytes) : "—", priv.airgap ? "Not on disk, and airgap is on: pre-seed the model cache first." : "Not on disk yet. Fetched once, when you start the first run.", "pend"),
      );
    } else list.push(row("Embedding model", "checking…", "", "wait"));
    if (agents) {
      const found = (agents.doctor || []).filter((c) => c.present || (c.files || []).some((f) => f.exists));
      list.push(row("Agent clients", `${found.length} found`, found.length ? found.map((c) => c.name).slice(0, 5).join(", ") + (found.length > 5 ? ` and ${found.length - 5} more` : "") : "None found yet — any MCP client can connect later"));
    } else list.push(row("Agent clients", "checking…", "", "wait"));
    list.push(row("Telemetry", "none", "No analytics and no update check of its own"));
    fill(checks, list);
    const waiting = list.filter((r) => r.querySelector(".spinner")).length;
    const pending = priv && !((priv.downloads || [])[0] || {}).cached;
    setText(checkLine, waiting ? "checking…" : pending ? "ready · 1 download pending" : "ready");
  };
  paint();
  for (const key of ["about", "accel", "privacy", "agents", "runs"]) load(key).then(paint).catch(paint);
  return node;
}

// A folder listing without the store home: indexing semlith's own stores
// is refused anyway, so offering them is a dead end.
function browsable(entries) {
  const home = data.about?.store_home;
  return (entries || []).filter((e) => !home || e.path !== home);
}

function laneWord(status) {
  const s = (status && status.state) || "idle";
  if (s === "compiling") return `compiling ${status.percent || 0}%`;
  if (s === "downloading") return `downloading ${status.percent || 0}%`;
  if (s === "ready" || s === "running" || s === "idle") return "ready";
  return s;
}

/** Adopt a `.semlith` somewhere under home: pick the folder, the daemon opens it. */
async function adoptFlow() {
  // The adopt runs inside the picker, so a refusal is shown there, at the same
  // folder, and the person can pick another without starting over.
  const out = await pickFolder({ title: "Adopt an existing store", ok: "Adopt this store", hint: "Folders holding a store are marked.", confirm: (path) => post("/api/adopt", { path }) });
  if (!out) return;
  toast(`Adopted ${out.name || out.store || "the store"}`);
  await load("stores", true);
  go("stores");
}

/** A modal folder browser over `/api/dirs`, confined to home by the daemon. */
function pickFolder({ title, ok, hint, start, confirm }) {
  return new Promise((resolve) => {
    let dir = start || "";
    let listing = null;
    const list = el("div", { class: "browse-grid" });
    const where = el("span", { class: "dir" });
    const up = btn({ class: "btn xs" }, icon(I.back, 13, { w: 1.8 }), "Up");
    const err = el("div", {});
    const modal = el(
      "div",
      { class: "modal wide", role: "dialog", "aria-modal": "true", "aria-label": title },
      el("div", { class: "mt", text: title }),
      hint ? el("div", { class: "mb", text: hint }) : null,
      el("div", { class: "card" }, el("div", { class: "browse-head" }, up, where), list),
      err,
      el("div", { class: "acts" }, btn({ class: "btn", onclick: () => done(null) }, "Cancel"), btn({ class: "btn primary", onclick: (e) => listing && choose(e.currentTarget) }, ok || "Use this folder")),
    );
    // Closed by Cancel, its close button or Escape only: a click outside, or
    // one that starts inside and ends outside, is not a choice.
    const scrim = el("div", { class: "modal-scrim" }, modal);
    const onKey = (e) => e.key === "Escape" && done(null);
    function done(v) {
      scrim.remove();
      document.removeEventListener("keydown", onKey, true);
      resolve(v);
    }
    async function choose(button) {
      if (!confirm) return done(listing.path);
      button.disabled = true;
      try {
        done(await confirm(listing.path));
      } catch (e) {
        toast(e.message, true);
        fill(err, errorBox(e.message));
        button.disabled = false;
      }
    }
    up.addEventListener("click", () => listing && listing.parent && open(listing.parent));
    async function open(path) {
      try {
        listing = await api(`/api/dirs?path=${encodeURIComponent(path || "")}`);
        dir = listing.path;
        state.home = state.home || listing.home;
        setText(where, tilde(dir, listing.home));
        up.disabled = !listing.parent;
        fill(err);
        fill(
          list,
          browsable(listing.entries)
            .filter((e) => e.dir)
            .map((e) =>
              el(
                "div",
                { class: "bitem" },
                btn({ class: "open", onclick: () => open(e.path) }, icon(I.folder, 14, { w: 1.6 }), el("span", { class: "name", text: e.name })),
                e.adoptable ? el("span", { class: "pill green sm", text: "a store" }) : null,
              ),
            ),
        );
      } catch (e) {
        fill(err, errorBox(e.message));
      }
    }
    document.body.append(scrim);
    document.addEventListener("keydown", onKey, true);
    open(dir);
  });
}

// -------------------------------------------------------------------- wizard

/* Name, Sources, Review, Index — and Connect on first run. Each step is real:
 * step 1 creates an empty named store, step 2 browses the daemon's filesystem
 * and resolves dropped items to their real paths, step 3 is the daemon's scan
 * held for review, step 4 is the run itself, step 5 registers clients. */
const NAME_RE = /^[a-z0-9][a-z0-9-]{0,39}$/;

// The scan's own phases (spec §2.3), as the wizard's scan card names them.
const SCAN_STAGES = [
  ["walk", "Walking the tree"],
  ["read", "Reading and hashing"],
  ["credentials", "Checking for credentials"],
  ["rules", "Matching rules"],
];

function blankWizard(opts) {
  const o = opts || {};
  return {
    onboarding: !!o.onboarding,
    existing: o.store || null,
    step: o.store ? 2 : 1,
    name: o.store || "",
    kind: "both",
    created: o.store || null,
    sources: [],
    mode: null,
    browse: { dir: "", listing: null, sel: new Set(), error: "" },
    pathDraft: "",
    urlDraft: "",
    split: "together",
    watch: true,
    gitignore: true,
    drag: false,
    scan: { state: "idle", runs: [], error: "" },
    decisions: {},
    decSel: new Set(),
    decShow: "all",
    decRisk: "any",
    open: {},
    started: [],
    record: true,
    watchAfter: true,
    tryQ: "",
    tried: null,
    clientSel: new Set(),
    reg: "idle",
    regResult: null,
    manual: false,
    nameSuggest: [],
  };
}

// What the summary says about sources: the new ones, and for an existing
// store what it already reads, so adding to it never reads as "none".
function sourcesLine() {
  const w = state.wz;
  const added = w.sources.map((x) => baseName(x.path)).join(", ");
  const had = w.existing ? (store(w.existing)?.roots || []).length : 0;
  if (!had) return added || "none yet";
  const kept = `${plural(had, "root")} already`;
  return added ? `${kept} + ${added}` : kept;
}

function openWizard(opts) {
  state.wz = blankWizard(opts);
  go("new");
}

function wizardScreen() {
  if (!state.wz) state.wz = blankWizard({});
  const w = state.wz;
  const host = el("div", { id: "app" });
  const top = el("header", { class: "top wide-pad" });
  const body = el("div", { class: "wz-scroll", "data-scroll-keep": "wz" });
  const foot = el("div", { class: "wz-foot" });
  host.append(top, body, foot);
  let paintGen = 0;

  // Data the steps read, fetched once.
  loadMany(["stores", "runs", "accel", "privacy", "agents"]).then(() => paint());

  function labels() {
    return ["Name", "Sources", "Review", "Index"].concat(w.onboarding ? ["Connect"] : []);
  }

  function run() {
    const ids = new Set(w.started);
    return (data.runs?.runs || []).filter((r) => ids.has(r.id));
  }
  function scanRuns() {
    const ids = new Set(w.scan.runs);
    return (data.runs?.runs || []).filter((r) => ids.has(r.id));
  }

  function plan() {
    const held = scanRuns().filter((r) => r.plan);
    const items = held.flatMap((r) => (r.plan.review || []).map((it) => ({ ...it, store: r.store, risk: riskOf(it) })));
    const notIndexed = {};
    const paths = {};
    for (const r of held) {
      for (const [cls, count] of Object.entries(r.plan.not_indexed || {})) notIndexed[cls] = (notIndexed[cls] || 0) + count;
      for (const [cls, list] of Object.entries(r.plan.not_indexed_paths || {})) paths[cls] = (paths[cls] || []).concat(list);
      if (r.plan.credential && r.plan.credential.length) paths.credential = (paths.credential || []).concat(r.plan.credential);
    }
    const decided = items.filter((d) => w.decisions[d.path]);
    const accepted = decided.filter((d) => w.decisions[d.path] === "in" || w.decisions[d.path] === "redact").length;
    const embed = held.reduce((a, r) => a + (r.plan.embed || 0), 0);
    const embedBytes = held.reduce((a, r) => a + (r.plan.embed_bytes || 0), 0);
    const unchanged = held.reduce((a, r) => a + (r.plan.unchanged || 0), 0);
    const eta = held.length && held.every((r) => r.plan.eta_ms != null) ? held.reduce((a, r) => a + r.plan.eta_ms, 0) : null;
    // A run-truth plan counts its chunks and images; an accepted file adds
    // its own chunks where the scan counted them.
    const chunks = held.length && held.every((r) => r.plan.chunks != null) ? held.reduce((a, r) => a + r.plan.chunks, 0) + decided.filter((d) => w.decisions[d.path] !== "out").reduce((a, d) => a + (d.chunks || 0), 0) : null;
    const images = held.reduce((a, r) => a + (r.plan.images || 0), 0);
    return {
      held,
      items,
      undecided: items.length - decided.length,
      accepted,
      chunks,
      images,
      embed,
      embedBytes,
      unchanged,
      eta,
      notIndexed,
      paths,
      credential: (notIndexed.credential || 0) || (paths.credential || []).length,
      skipped: Object.entries(notIndexed).filter(([c]) => c !== "credential" && c !== "content" && c !== "policy").reduce((a, [, v]) => a + v, 0),
    };
  }

  /** The run's estimate before Start (spec §3.3): the plan's chunks over the
   * summed rates of the lanes that are on, plus its images over CLIP's rate.
   * Lanes come from /api/accel, so it moves as a lane is switched. An older
   * daemon counts no chunks: its text bytes over about 1 KB a chunk, or its
   * own per-store byte rate when no lane has a rate either. */
  function estimate(P) {
    const accel = data.accel || {};
    const rates = accel.rates || {};
    const lanes = (accel.lanes || []).filter((l) => (l.status?.state || "") !== "unavailable");
    const on = lanes.filter((l) => l.enabled);
    if (accel.cpu_fallback && !on.some((l) => l.lane === "cpu")) on.push(...lanes.filter((l) => l.lane === "cpu"));
    let perSec = 0;
    let knownAnswer = false;
    for (const l of on) {
      const r = rates[l.lane];
      const v = r && r.chunks_per_s > 0 ? r.chunks_per_s : l.rate > 0 ? l.rate : 0;
      if (r && r.chunks_per_s > 0 && r.source === "known-answer") knownAnswer = true;
      perSec += v;
    }
    // ponytail: 1 KB a chunk is the bench median (spec §1.2); only an older daemon needs it.
    const chunks = P.chunks != null ? P.chunks : P.embedBytes ? P.embedBytes / 1024 : null;
    if (perSec > 0 && chunks != null) {
      const clip = rates.clip && rates.clip.images_per_s;
      return { ms: (chunks / perSec) * 1000 + (P.images && clip > 0 ? (P.images / clip) * 1000 : 0), knownAnswer };
    }
    return P.eta != null ? { ms: P.eta, knownAnswer: false } : null;
  }

  function estimateText(e) {
    return e ? `${e.ms < 60000 ? "under a minute" : spellAbout(e.ms)} on this machine${e.knownAnswer ? " (from the lane's known-answer speed)" : ""}` : null;
  }

  function nameState() {
    const nm = w.name.trim();
    const taken = (data.stores?.stores || []).some((s) => s.name === nm && s.name !== w.created);
    const fmtOk = NAME_RE.test(nm);
    const ok = fmtOk && !taken;
    const msg = !nm
      ? ["Lowercase letters, digits and dashes. Agents see this name when they choose where to look.", ""]
      : !fmtOk
        ? ["Use lowercase letters, digits and dashes — for example research-notes.", "bad"]
        : taken
          ? [`“${nm}” is already a store on this machine. Pick another name.`, "bad"]
          : [`Available · kept at ~/.semlith/stores/${nm}`, "ok"];
    return { nm, ok, msg };
  }

  async function exit() {
    if (w.onboarding && !w.created) {
      state.wz = null;
      return go("welcome");
    }
    const r = run().find((x) => LIVE_RUN.has(x.status) && x.status !== "review");
    if (w.created && !w.existing && !r && !(store(w.created) && store(w.created).files)) {
      const del = await ask({
        title: "Leave without indexing?",
        body: `The store ${w.created} stays, empty. Add sources from its page whenever you're ready, or delete it now.`,
        ok: "Delete store",
        cancel: "Keep it",
        danger: true,
      });
      if (del) {
        for (const s of scanRuns()) await post("/api/index/control", { store: s.store, run: s.id, action: "stop", delete: true }).catch(() => {});
        await act(() => post("/api/store/delete", { store: w.created }), `Deleted ${w.created}`);
        await load("stores", true);
      } else {
        // A scan held for review is dropped either way: leaving is not a
        // decision to index what it found.
        for (const s of scanRuns().filter((x) => x.status === "review")) await post("/api/index/control", { store: s.store, run: s.id, action: "stop" }).catch(() => {});
      }
    }
    state.wz = null;
    if (w.created && (store(w.created) || w.existing)) go("store", w.created, r ? "runs" : "overview");
    else go(liveStores().length ? "home" : "welcome");
  }

  function paint() {
    const mine = ++paintGen;
    if (!host.isConnected && mine > 1) return;
    const L = labels();
    const step = w.step;
    const R = run();
    const live = R.find((x) => LIVE_RUN.has(x.status));
    const doneRun = R.length && R.every((x) => !LIVE_RUN.has(x.status));
    const maxStep = w.created ? (w.sources.length ? (w.scan.state === "done" ? (R.length ? L.length : 4) : 3) : 2) : 1;
    fill(
      top,
      logo(),
      el("span", { class: "wordmark hide-sm", text: "Semlith" }),
      el("span", { class: "vrule hide-sm" }),
      el("span", { class: "hide-sm ink2 nowrap", text: w.onboarding ? "Set up Semlith" : w.existing ? `Add sources to ${w.existing}` : "New store" }),
      el(
        "div",
        { class: "wz-steps" },
        L.map((label, i) => {
          const num = i + 1;
          const done = num < step;
          const cur = num === step;
          const reach = num <= maxStep && num !== step && !(live && num < 4 && live.status !== "review");
          return el(
            "div",
            { class: "wz-step" },
            i ? el("span", { class: `ln${num <= step ? " done" : ""}` }) : null,
            btn(
              { disabled: reach ? null : true, "aria-current": cur ? "step" : null, onclick: () => reach && setStep(num) },
              el("span", { class: `wz-circle${done ? " done" : cur ? " cur" : ""}` }, done ? icon(I.check, 11, { w: 3 }) : String(num)),
              el("span", { class: `lab hide-sm${cur ? " cur" : done ? " done" : ""}`, text: label }),
            ),
          );
        }),
      ),
      btn({ class: "btn", onclick: exit }, w.onboarding ? (step === 1 ? "Back to welcome" : "Exit setup") : "Cancel"),
    );
    const heads = {
      1: ["Name your store", "A store is one index on this machine. You can add more sources to it later, and make as many stores as you like."],
      2: ["Add what it should read", "Drop folders or files, browse to them, or paste a path. Semlith reads them in place and watches them for changes."],
      3: [w.scan.state === "scanning" || w.scan.state === "idle" ? "Scanning…" : plan().undecided ? "Review before anything is indexed" : "Everything is decided","Semlith checked every file for credentials and generated noise. Nothing has been embedded yet — this is your chance to say no."],
      4: [R.length ? (doneRun ? "Indexed" : `Indexing ${w.created}`) : "Ready to index", R.length ? "Chunking, embedding and writing the graph — all on this machine." : "Where the work runs, and what to keep doing after. The defaults suit this machine."],
      5: ["Connect your agents", `One endpoint, ${data.agents?.endpoint?.url || "on this machine"}, for every client. Semlith writes each client's config for you.`],
    }[step];
    const left = el(
      "div",
      { class: "stack" },
      el("div", { class: "wz-head" }, el("span", { class: "eyebrow", text: `STEP ${step} OF ${L.length}` }), el("div", { class: "h", text: heads[0] }), el("div", { class: "lead", text: heads[1] })),
      [null, step1, step2, step3, step4, step5][step](),
    );
    // Replacing the body empties it for a moment, which put the scroll back
    // at the top on every decision; the offsets, the page's and any inner
    // list's, are put back once the new body is in.
    const at = body.scrollTop;
    const inner = [...body.querySelectorAll("[data-scroll-keep]")].map((nd) => [nd.getAttribute("data-scroll-keep"), nd.scrollTop]);
    fill(body, el("div", { class: "wz-body" }, left, summaryRail()));
    body.scrollTop = at;
    for (const [k, t] of inner) {
      const nd = body.querySelector(`[data-scroll-keep="${k}"]`);
      if (nd) nd.scrollTop = t;
    }
    paintFoot();
  }

  function setStep(n) {
    w.step = n;
    body.scrollTop = 0;
    paint();
  }

  // ---- step 1: name
  function step1() {
    const ns = nameState();
    const input = el("input", {
      value: w.name,
      placeholder: "e.g. docs, api, research-notes",
      spellcheck: "false",
      autocomplete: "off",
      "aria-label": "Store name",
      "data-keep": "wz-name",
      oninput: (e) => {
        const v = e.target.value.toLowerCase().replace(/\s+/g, "-");
        if (v !== e.target.value) e.target.value = v;
        w.name = v;
        const s = nameState();
        box.className = `namebox${!s.nm ? "" : s.ok ? " ok" : " bad"}`;
        setText(hint, s.msg[0]);
        hint.className = `hint-l${s.msg[1] ? ` ${s.msg[1]}` : ""}`;
        tick.hidden = !s.ok;
        paintFoot();
        paintRail();
      },
      onkeydown: (e) => e.key === "Enter" && next(),
    });
    const tick = el("span", { class: "check-ok", hidden: !ns.ok }, icon(I.check, 11, { w: 3 }));
    const box = el("div", { class: `namebox${!ns.nm ? "" : ns.ok ? " ok" : " bad"}` }, icon(I.layers, 16, { w: 1.6 }), input, tick);
    const hint = el("div", { class: `hint-l${ns.msg[1] ? ` ${ns.msg[1]}` : ""}`, text: ns.msg[0] });
    setTimeout(() => input.focus(), 0);
    const suggestRow = el("div", { class: "row gap6" });
    paintSuggest(suggestRow, input);
    return el(
      "div",
      { class: "stack" },
      el("div", { class: "card pad16" }, el("div", { class: "field-l" }, el("span", { class: "eyebrow", text: "Store name" }), box, hint), suggestRow),
      el(
        "div",
        { class: "card pad16" },
        el("div", { class: "row base" }, el("span", { class: "card-t grow", text: "What will it hold?" }), meta("sets how search ranks results · change it any time")),
        el(
          "div",
          { class: "auto-fit m180" },
          [
            ["code", "Code", "Repositories. Symbols and call edges are extracted, and search leans to code.", `tree-sitter · ${data.about?.languages || 46} languages`],
            ["docs", "Docs & notes", "Markdown, PDF, Office, slides and notebooks. Search leans to prose.", "text · pdf · office · notebook"],
            ["both", "Both", "Code beside its docs — the usual repository. Search weighs them equally.", "every reader", true],
          ].map(([k, t, d, m, rec]) =>
            btn(
              { class: `pick-card${w.kind === k ? " on" : ""}`, "aria-pressed": String(w.kind === k), onclick: () => ((w.kind = k), paint()) },
              el("div", { class: "row nowrap" }, el("span", { class: `radio${w.kind === k ? " on" : ""}` }), el("span", { class: "t", text: t }), rec ? el("span", { class: "rec", text: "usual" }) : null),
              el("span", { class: "d", text: d }),
              el("span", { class: "m", text: m }),
            ),
          ),
        ),
      ),
    );
  }

  async function paintSuggest(row, input) {
    // Names from the folders this person keeps under home, not a fixed list.
    try {
      if (!w.nameSuggest.length) {
        const listing = await api("/api/dirs");
        state.home = state.home || listing.home;
        w.nameSuggest = listing.entries
          .filter((e) => e.dir)
          .map((e) => e.name.toLowerCase().replace(/[^a-z0-9-]+/g, "-").replace(/^-+|-+$/g, ""))
          .filter((x) => NAME_RE.test(x) && !["library", "applications", "desktop", "downloads", "movies", "music", "pictures", "public"].includes(x));
      }
    } catch (_) {
      return;
    }
    const taken = new Set((data.stores?.stores || []).map((s) => s.name));
    const list = w.nameSuggest.filter((x) => !taken.has(x) && x !== w.name).slice(0, 4);
    if (!list.length) return;
    fill(
      row,
      el("span", { class: "muted t-sm", text: "From folders you work in" }),
      list.map((x) =>
        btn(
          {
            class: "chip xs soft",
            onclick: () => {
              w.name = x;
              input.value = x;
              input.dispatchEvent(new Event("input"));
            },
          },
          x,
        ),
      ),
    );
  }

  // ---- step 2: sources
  function step2() {
    // A drop goes through three ways of finding where it sits, in order:
    //  1. The real path, when the browser gives one: a drag that carries
    //     path text (any browser), or Safari, which writes a dropped item's
    //     path into a text field — the unseen catcher over the whole zone.
    //  2. The daemon lookup, from the item's name, size and time.
    //  3. The paste box, when the lookup finds nothing.
    // In Safari the zone never accepts the drag itself: Safari decides while
    // the drag moves whether a drop is text for a field, and a page that
    // accepted it on the way in got a drop with no path in it. Chrome and
    // Firefox write no path into a field and, unless the page accepts the
    // drag, deliver no drop at all, so there the zone accepts it and goes
    // straight to the lookup.
    const native = /^Apple/.test(navigator.vendor || "");
    const catcher = el("input", {
      type: "text",
      class: "drop-catch",
      tabindex: "-1",
      "aria-hidden": "true",
      spellcheck: "false",
      autocomplete: "off",
      // Not a place to type: a click on the zone lands here.
      onkeydown: (e) => !(e.metaKey || e.ctrlKey) && e.preventDefault(),
      oninput: () => {
        const text = catcher.value;
        catcher.value = "";
        endDrag();
        if (!/(^|\s)(file:\/\/|\/|~\/|[A-Za-z]:\\)/.test(text)) return;
        clearTimeout(w.dropWait);
        w.dropWait = null;
        addPasted(splitDropped(text));
      },
    });
    const endDrag = () => {
      w.drag = false;
      zone.classList.remove("drag");
      setText(zoneTitle, "Drop folders or files here");
    };
    const zone = el(
      "div",
      {
        class: `dropzone${w.drag ? " drag" : ""}${native ? " native" : ""}`,
        ondragover: (e) => {
          if (!native) e.preventDefault();
          if (!w.drag) {
            w.drag = true;
            zone.classList.add("drag");
            setText(zoneTitle, "Let go to add");
          }
        },
        ondragleave: (e) => {
          if (zone.contains(e.relatedTarget)) return;
          w.drag = false;
          zone.classList.remove("drag");
          setText(zoneTitle, "Drop folders or files here");
        },
        ondrop: (e) => {
          endDrag();
          // Paths the drag carries as text are exact, in every browser.
          const text = e.dataTransfer && (e.dataTransfer.getData("text/uri-list") || e.dataTransfer.getData("text/plain"));
          if (text && /^(file:\/\/|\/|~\/|[A-Za-z]:\\)/m.test(text)) {
            e.preventDefault();
            clearTimeout(w.dropWait);
            return addPasted(splitDropped(text));
          }
          // What the lookup needs has to be read now, while the drop lasts.
          const taken = takeDrop(e.dataTransfer);
          if (!native) {
            e.preventDefault();
            return resolveDrop(taken);
          }
          // Safari: not prevented, so it writes the paths into the catcher,
          // whose input event adds them and cancels the lookup that otherwise
          // runs after a moment.
          catcher.value = "";
          clearTimeout(w.dropWait);
          w.dropWait = setTimeout(() => {
            w.dropWait = null;
            resolveDrop(taken);
          }, 400);
        },
      },
      catcher,
      el("span", { class: "icon-tile" }, icon(I.upload, 19, { w: 1.7 })),
    );
    const zoneTitle = el("span", { class: "t", text: w.drag ? "Let go to add" : "Drop folders or files here" });
    zone.append(
      el("div", { class: "col gap2 center" }, zoneTitle, el("span", { class: "d", text: "Folders, single files or a mix. Nothing is copied or uploaded — semlith reads them where they sit." })),
      el(
        "div",
        { class: "row" },
        [
          ["browse", "Browse folders", I.folder],
          ["paste", "Paste a path", I.paste],
          ["url", "Add a URL", I.link],
        ].map(([m, label, d]) =>
          btn({ class: `btn sm${w.mode === m ? " dark" : ""}`, "aria-pressed": String(w.mode === m), onclick: () => ((w.mode = w.mode === m ? null : m), paint()) }, icon(d, 14), label),
        ),
      ),
    );
    const parts = [zone];
    if (w.mode === "browse") parts.push(browsePanel());
    if (w.mode === "paste") parts.push(pastePanel());
    if (w.mode === "url") parts.push(urlPanel());
    const multi = w.sources.find((s) => s.repos > 1);
    if (multi)
      parts.push(
        el(
          "div",
          { class: "notice blue" },
          icon(I.info, 17, { w: 1.7 }),
          el("div", { class: "body" }, el("span", { class: "ttl", text: `${tilde(multi.path)} holds ${multi.repos} repositories` }), el("span", { class: "sub", text: "Keep them in one store to search across them, or give each its own store so agents can aim at one." })),
          el(
            "div",
            { class: "row gap6" },
            [
              ["together", "Keep together"],
              ["split", "One store each"],
            ].map(([v, label]) =>
              btn({ class: "chip", "aria-pressed": String(w.split === v), onclick: () => ((w.split = v), v === "split" && toast("Each repository becomes its own store, named after its folder"), paint()) }, label),
            ),
          ),
        ),
      );
    parts.push(
      el(
        "div",
        { class: "card" },
        el("div", { class: "card-h" }, el("span", { class: "card-t", text: "Sources" }), el("span", { class: "count n20 ink", text: String(w.sources.length) }), el("span", { class: "spacer" }), meta(w.sources.length ? sourceTotal() : "")),
        !w.sources.length ? empty("Nothing added yet. Drop something above, or pick a folder.", "lg") : null,
        w.sources.map((s, i) =>
          el(
            "div",
            { class: "src-row" },
            el("span", { class: "icon-tile s28" }, icon(s.type === "url" ? I.link : s.type === "file" ? I.file : I.folder, 14, { w: 1.6 })),
            el("div", { class: "col" }, pathSpan(s.type === "url" ? s.path : tilde(s.path), "p", s.path), el("span", { class: "m", text: sourceMeta(s) })),
            el("div", { class: "row gap6" }, el("span", { class: "tag", text: s.type }), s.repos === 1 ? el("span", { class: "tag", text: "git" }) : null),
            btn({ class: "x-btn", "aria-label": `Remove ${s.path}`, "data-tip": "Remove", onclick: () => (w.sources.splice(i, 1), paint()) }, icon(I.x, 13, { w: 2 })),
          ),
        ),
        el(
          "div",
          { class: "toggles-foot" },
          toggleRow(w.watch, "Watch for changes", "Re-index a file the moment it is saved.", (v) => ((w.watch = v), (w.watchAfter = v), paint())),
          toggleRow(w.gitignore, "Respect .gitignore", "Skip what each repository already ignores.", (v) => ((w.gitignore = v), paint())),
        ),
      ),
    );
    return el("div", { class: "stack", onpaste: onPaste }, parts);
  }

  function sourceTotal() {
    const folders = w.sources.filter((s) => s.type === "folder").length;
    const files = w.sources.filter((s) => s.type === "file").length;
    const urls = w.sources.filter((s) => s.type === "url").length;
    return [folders && plural(folders, "folder"), files && plural(files, "file"), urls && plural(urls, "URL")].filter(Boolean).join(" · ");
  }

  function sourceMeta(s) {
    if (s.type === "url") return "one https request · kept in the store's downloads folder";
    if (s.type === "file") return s.dropped ? "dropped file · read where it sits" : "single file · read where it sits";
    const bits = [];
    if (s.repos > 1) bits.push(`${s.repos} repositories inside`);
    else if (s.repos === 1) bits.push("a git repository");
    if (s.count) bits.push(`${plural(s.count, "folder")} inside`);
    bits.push(s.dropped ? "dropped · read in place" : "read in place");
    return bits.join(" · ");
  }

  async function addSources(list) {
    const have = new Set(w.sources.map((s) => s.path));
    const fresh = list.filter((s) => s.path && !have.has(s.path));
    if (!fresh.length) return;
    w.sources.push(...fresh);
    toast(fresh.length === 1 ? `Added ${baseName(fresh[0].path)}` : `Added ${fresh.length} sources`);
    paint();
    // What is inside each folder: repositories, by the daemon's own discovery,
    // so "Keep together / One store each" is offered only where it is true.
    for (const s of fresh.filter((x) => x.type === "folder")) {
      try {
        const found = await api(`/api/projects?path=${encodeURIComponent(s.path)}`);
        s.repos = found.repositories ? (found.projects || []).length : 0;
        s.repoPaths = found.repositories ? (found.projects || []).map((p) => p.path) : [];
        s.count = (found.folders || []).length;
      } catch (_) {
        /* the path may be outside home; the scan is the real test */
      }
    }
    paint();
  }

  function browsePanel() {
    const b = w.browse;
    if (!b.listing && !b.loading) {
      b.loading = true;
      api(`/api/dirs?path=${encodeURIComponent(b.dir || "")}`)
        .then((l) => {
          b.listing = l;
          b.dir = l.path;
          state.home = state.home || l.home;
        })
        .catch((e) => (b.error = e.message))
        .finally(() => {
          b.loading = false;
          paint();
        });
    }
    const l = b.listing;
    const openDir = (path) => {
      b.dir = path;
      b.listing = null;
      paint();
    };
    // A selection is drawn in place, not by repainting: a repaint between the
    // two clicks of a double-click replaces the item, and the browser then
    // sees two single clicks on two elements and no double-click.
    const count = meta("");
    const addBtn = btn(
      {
        class: "btn sm dark",
        disabled: !l ? true : null,
        onclick: () => {
          const list = b.sel.size ? [...b.sel] : [l.path];
          b.sel.clear();
          w.mode = null;
          addSources(list.map((p) => ({ path: p.path || p, type: p.type || "folder" })));
        },
      },
      "",
    );
    const syncHead = () => {
      count.textContent = `${b.sel.size} selected`;
      count.hidden = !b.sel.size;
      addBtn.textContent = b.sel.size ? `Add ${plural(b.sel.size, "item")}` : "Use this folder";
    };
    syncHead();
    return el(
      "div",
      { class: "card" },
      el(
        "div",
        { class: "browse-head" },
        btn({ class: "btn xs", disabled: !l || !l.parent ? true : null, onclick: () => l && l.parent && openDir(l.parent) }, icon(I.back, 13, { w: 1.8 }), "Up"),
        el("span", { class: "dir", text: l ? tilde(l.path, l.home) : "…" }),
        count,
        addBtn,
      ),
      el("div", { class: "browse-hint", text: "Click to select · double-click a folder to open it" }),
      b.error ? errorBox(b.error) : null,
      l
        ? el(
            "div",
            { class: "browse-grid", "data-scroll-keep": "browse" },
            browsable(l.entries).length
              ? browsable(l.entries).map((e) => {
                  const on = [...b.sel].some((x) => x.path === e.path);
                  const toggleSel = () => {
                    const hit = [...b.sel].find((x) => x.path === e.path);
                    if (hit) b.sel.delete(hit);
                    else b.sel.add({ path: e.path, type: e.dir ? "folder" : "file" });
                    const now = !hit;
                    item.classList.toggle("on", now);
                    cb.setAttribute("aria-checked", String(now));
                    syncHead();
                  };
                  const cb = checkbox(on, toggleSel, `Select ${e.name}`);
                  const item = el(
                    "div",
                    { class: `bitem${on ? " on" : ""}${e.dir ? "" : " file"}` },
                    cb,
                    btn(
                      {
                        class: "open",
                        "data-tip": e.dir ? `${e.name} — double-click to open` : e.name,
                        onclick: toggleSel,
                        // The two clicks before it toggled the item twice, so
                        // the selection is as it was when the folder opens.
                        ondblclick: () => e.dir && openDir(e.path),
                      },
                      icon(e.dir ? I.folder : I.file, 14, { w: 1.6 }),
                      el("span", { class: "name", text: e.name }),
                    ),
                    e.adoptable ? el("span", { class: "note", text: "store" }) : null,
                  );
                  return item;
                })
              : empty("Nothing here."),
          )
        : el("div", { class: "empty" }, "Reading…"),
    );
  }

  function pastePanel() {
    const input = el("input", {
      value: w.pathDraft,
      placeholder: "~/work/api — Enter adds it, several lines add several",
      spellcheck: "false",
      "data-keep": "wz-path",
      oninput: (e) => {
        w.pathDraft = e.target.value;
        addBtn.disabled = !w.pathDraft.trim();
      },
      onkeydown: (e) => e.key === "Enter" && addPasted(w.pathDraft),
      onpaste: (e) => {
        const text = e.clipboardData && e.clipboardData.getData("text");
        if (text && text.includes("\n")) {
          e.preventDefault();
          addPasted(text);
        }
      },
    });
    const addBtn = btn({ class: "btn md dark", disabled: !w.pathDraft.trim() ? true : null, onclick: () => addPasted(w.pathDraft) }, "Add");
    setTimeout(() => input.focus(), 0);
    const known = [...new Set(liveStores().flatMap((s) => (s.roots || []).map((r) => r.path)))].filter((p) => !w.sources.some((x) => x.path === p)).slice(0, 4);
    return el(
      "div",
      { class: "card pad" },
      el("div", { class: "row nowrap" }, el("div", { class: "box h36 focus grow" }, el("span", { class: "lab", text: "path" }), input), addBtn),
      known.length
        ? el(
            "div",
            { class: "col" },
            el("span", { class: "eyebrow sm", text: "Folders other stores already read" }),
            known.map((p) => btn({ class: "suggest-row", onclick: () => addSources([{ path: p, type: "folder" }]) }, icon(I.folder, 13, { w: 1.6 }), pathSpan(tilde(p), "grow", p), el("span", { class: "note", text: "folder" }))),
          )
        : null,
    );
  }

  /* Path text, as people paste it: quoted, `file://` URLs, a `~`, one per line. */
  async function addPasted(text) {
    const lines = String(text || "")
      .split(/\r?\n/)
      .map((x) => x.trim().replace(/^['"]|['"]$/g, ""))
      .filter(Boolean)
      .map((x) => {
        if (x.startsWith("file://")) {
          try {
            x = decodeURIComponent(new URL(x).pathname);
            if (/^\/[A-Za-z]:\//.test(x)) x = x.slice(1);
          } catch (_) {
            /* left as typed */
          }
        }
        if (x === "~" || x.startsWith("~/")) x = (state.home || "") + x.slice(1);
        return x;
      });
    if (!lines.length) return;
    w.pathDraft = "";
    const resolved = [];
    for (const p of lines) {
      if (/^https:\/\//.test(p)) {
        resolved.push({ path: p, type: "url" });
        continue;
      }
      // Asking the daemon whether it is a folder it can list; anything else is
      // taken as a file, and the scan says if it is not readable.
      let type = "file";
      try {
        await api(`/api/dirs?path=${encodeURIComponent(p)}`);
        type = "folder";
      } catch (e) {
        if (/outside the home/.test(e.message)) {
          toast(`${p} is outside your home folder, which the portal may not read`, true);
          continue;
        }
      }
      resolved.push({ path: p, type });
    }
    addSources(resolved);
  }

  function onPaste(e) {
    if (e.target && e.target.matches && e.target.matches("input, textarea")) return;
    const text = e.clipboardData && (e.clipboardData.getData("text/uri-list") || e.clipboardData.getData("text"));
    if (text && /[\\/~]|^file:/.test(text)) {
      e.preventDefault();
      addPasted(text);
    }
  }

  function urlPanel() {
    const ok = () => /^https:\/\/\S+\.\S+/.test(w.urlDraft.trim());
    const input = el("input", {
      value: w.urlDraft,
      placeholder: "A web address: a page, a PDF or a file on GitHub",
      spellcheck: "false",
      "data-keep": "wz-url",
      oninput: (e) => {
        w.urlDraft = e.target.value;
        fetchBtn.disabled = !ok();
      },
      onkeydown: (e) => e.key === "Enter" && ok() && addUrl(),
    });
    const addUrl = () => {
      addSources([{ path: w.urlDraft.trim(), type: "url" }]);
      w.urlDraft = "";
    };
    const fetchBtn = btn({ class: "btn md dark", disabled: !ok() ? true : null, onclick: addUrl }, "Fetch");
    setTimeout(() => input.focus(), 0);
    const airgap = data.privacy?.airgap;
    return el(
      "div",
      { class: "card pad" },
      el("div", { class: "row nowrap" }, el("div", { class: "box h36 focus grow" }, el("span", { class: "lab", text: "url" }), input), fetchBtn),
      el("div", { class: "muted t-sm pretty", text: "One https request for exactly this URL, made when the run starts. Nothing is crawled and no credential is sent. The file is kept in this store's downloads folder, never in your working tree." }),
      airgap && (airgap === true || airgap.on) ? el("div", { class: "notice amber" }, el("span", { class: "sub", text: "Airgap is on, so this fetch will be refused. Turn it off on the Privacy page first." })) : null,
    );
  }

  /* Drag and drop with real paths. No browser tells a page where a dropped
   * item lives, and nothing is uploaded: the page sends each item's name, kind,
   * size, time and — for a folder — its first-level names and a few file sizes,
   * and the daemon finds the one place on this machine that matches. One match
   * is added; several give a picker; none points at the path box. */
  // Several dropped items written into one field: one per line, or file://
  // URLs one after another on a line.
  function splitDropped(text) {
    return String(text || "")
      .split(/\r?\n/)
      // Written one after another on a line: file:// URLs, or absolute paths,
      // each starting where a space is followed by a slash or a drive.
      .flatMap((line) => line.trim().split(/\s+(?=file:\/\/|\/|[A-Za-z]:\\)/))
      .filter(Boolean)
      .join("\n");
  }



  // The drop's items, read while the event lasts: a DataTransfer is emptied
  // the moment its event returns.
  function takeDrop(transfer) {
    const entries = [];
    for (const it of Array.from((transfer && transfer.items) || [])) {
      if (it.kind !== "file") continue;
      const entry = it.webkitGetAsEntry ? it.webkitGetAsEntry() : null;
      const file = it.getAsFile ? it.getAsFile() : null;
      entries.push({ entry, file });
    }
    return entries;
  }

  async function resolveDrop(entries) {
    const items = [];
    if (!entries.length) return;
    for (const { entry, file } of entries) {
      const name = entry ? entry.name : file ? file.name : "";
      if (!name) continue;
      const dir = entry ? entry.isDirectory : false;
      const item = { name, kind: dir ? "dir" : "file" };
      if (file && !dir) {
        item.size = file.size;
        item.mtime = file.lastModified;
      }
      if (dir && entry.createReader) {
        try {
          const kids = await readEntries(entry.createReader());
          item.children = kids.slice(0, 200).map((k) => k.name);
          const files = kids.filter((k) => k.isFile).slice(0, 20);
          item.child_files = (await Promise.all(files.map((f) => new Promise((res) => f.file((x) => res({ name: x.name, size: x.size, mtime: x.lastModified }), () => res(null)))))).filter(Boolean);
        } catch (_) {
          /* a folder the browser would not list still resolves by name */
        }
      }
      items.push(item);
    }
    if (!items.length) return;
    toast(`Finding ${items.length === 1 ? items[0].name : `${items.length} items`} on this machine…`);
    let answer;
    try {
      answer = await post("/api/drop/resolve", { items, ...(state.pasteboardChange != null ? { pasteboard_change: state.pasteboardChange } : {}) });
      if (answer.pasteboard_change != null) state.pasteboardChange = answer.pasteboard_change;
    } catch (e) {
      toast(`Could not resolve the drop: ${e.message}`, true);
      w.mode = "paste";
      return paint();
    }
    const add = [];
    const unresolved = [];
    for (const r of answer.results || []) {
      if (r.status === "resolved" && r.path) add.push({ path: r.path, type: items.find((i) => i.name === r.name)?.kind === "dir" ? "folder" : "file", dropped: true });
      else if (r.status === "ambiguous" && (r.candidates || []).length) {
        const pick = await choosePath(r.name, r.candidates);
        if (pick) add.push({ path: pick, type: items.find((i) => i.name === r.name)?.kind === "dir" ? "folder" : "file", dropped: true });
      } else if (r.status === "refused") toast(`${r.name}: ${r.reason || "it sits in a temporary folder — extract it first"}`, true);
      else unresolved.push(r.name);
    }
    if (add.length) {
      // The person confirms every resolved path: it is shown before it is used.
      const ok = await ask({
        title: add.length === 1 ? "Add this path?" : `Add these ${add.length} paths?`,
        body: "Found on this machine from what was dropped. Nothing was uploaded.",
        extra: el("div", { class: "col gap4" }, add.map((a) => el("div", { class: "copyfield" }, pathSpan(a.path, "t")))),
        ok: add.length === 1 ? "Add it" : "Add them",
      });
      if (ok) addSources(add);
    }
    if (unresolved.length) {
      w.mode = "paste";
      paint();
      toast(`Could not find ${unresolved.join(", ")} — paste its path (${answer.os_copy_hint || "copy it from your file manager"})`, true);
    }
  }

  function choosePath(name, candidates) {
    return new Promise((resolve) => {
      let picked = candidates[0];
      const list = el(
        "div",
        { class: "col gap4" },
        candidates.map((c, i) =>
          btn(
            {
              class: "modal opt",
              onclick: (e) => {
                picked = c;
                for (const b of list.children) b.querySelector(".radio").classList.toggle("on", b === e.currentTarget);
              },
            },
            el("span", { class: `radio${i === 0 ? " on" : ""}` }),
            el("span", { class: "mono anywhere", text: c }),
          ),
        ),
      );
      ask({ title: `Which ${name}?`, body: "More than one place on this machine matches what was dropped.", extra: list, ok: "Use this one", wide: true }).then((ok) => resolve(ok ? picked : null));
    });
  }

  function readEntries(reader) {
    return new Promise((resolve, reject) => {
      const all = [];
      const more = () =>
        reader.readEntries((batch) => {
          if (!batch.length || all.length >= 200) return resolve(all);
          all.push(...batch);
          more();
        }, reject);
      more();
    });
  }

  // ---- step 3: review
  function step3() {
    if (w.scan.state === "idle") startScan();
    if (w.scan.state === "error") return el("div", { class: "card" }, errorBox(w.scan.error), el("div", { class: "card-b" }, el("div", { class: "row" }, btn({ class: "btn", onclick: () => setStep(2) }, "Back to sources"), btn({ class: "btn primary", onclick: () => ((w.scan.state = "idle"), paint()) }, "Scan again"))));
    const runs = scanRuns();
    const scanning = w.scan.state !== "done";
    if (scanning) {
      const sum = (k) => runs.reduce((a, r) => a + (r[k] || 0), 0);
      const total = sum("total");
      const scanned = sum("scanned");
      // The stages are the scan's own phases (spec §2.3), each shown for at
      // least DWELL_MS so a scan over in a blink still reads as four steps;
      // the card never runs ahead of the scan.
      const shown = Math.min(w.scan.shown || 0, SCAN_STAGES.length);
      const real = scanReal();
      const counter = {
        walk: total ? `${plural(total, "file")} found` : "looking…",
        read: total ? `${n(scanned)} / ${n(total)}` : "reading…",
        credentials: runs.some((r) => r.phase === "credentials") && total ? `${n(scanned)} / ${n(total)} checked` : "content + names",
        rules: ".gitignore · build",
      };
      const frac = shown === real && SCAN_STAGES[shown] && SCAN_STAGES[shown][0] === "read" && total ? scanned / total : 0;
      const p = Math.max(3, ((shown + frac) / SCAN_STAGES.length) * 100);
      const stages = SCAN_STAGES.map(([key, label], k) => [label, k < shown, k === shown, counter[key], key]);
      const bytesTip = sum("bytes_total") ? `${bytes(sum("bytes"))} of ${bytes(sum("bytes_total"))} read` : null;
      paceScan();
      return el(
        "div",
        { class: "card pad16" },
        el("div", { class: "row base" }, el("span", { class: "card-t grow", text: `Scanning ${plural(w.sources.filter((s) => s.type !== "url").length, "source")}` }), el("span", { class: "mono t-b", text: `${Math.floor(p)}%` })),
        (() => {
          const b = bar(p, "h8");
          b.setAttribute("data-tip", `Scan · ${Math.floor(p)}%`);
          b.setAttribute("data-tip-rows", stages.map((s) => `${s[0]}::${s[1] ? "done" : s[2] ? s[3] : "waiting"}`).join("||"));
          return b;
        })(),
        el(
          "div",
          { class: "auto-fit m170 gap8" },
          stages.map(([label, done, cur, m, key]) =>
            el(
              "div",
              { class: `stage-box${done ? " done" : cur ? " cur" : ""}`, "data-stage": key, "data-tip": key === "read" ? bytesTip : null },
              el("span", { class: `dot ${done ? "green" : cur ? "blue pulse" : "line"}` }),
              el("span", { class: "sb-text" }, el("span", { class: "sb-label", text: label }), el("span", { class: "sb-status", text: done ? "done" : cur ? m : "waiting" })),
            ),
          ),
        ),
        el("div", { class: "muted t-sm", text: "Names and hashes only. Nothing is embedded until you start the run." }),
      );
    }
    return reviewPanel();
  }

  /** How far the scan really is, in stages: the least advanced of its runs,
   * by the phase each names (an older daemon names none: walking until it
   * has a total, then reading). Every stage once the scan is held. */
  function scanReal() {
    if (w.scan.realDone) return SCAN_STAGES.length;
    const live = scanRuns().filter((r) => r.status !== "review" && LIVE_RUN.has(r.status));
    if (!live.length) return 0;
    const at = (r) => {
      const k = SCAN_STAGES.findIndex(([key]) => key === r.phase);
      return k >= 0 ? k : r.total ? 1 : 0;
    };
    return Math.min(...live.map(at));
  }

  // While a scan is shown, the card is redrawn a few times a second so each
  // stage it has reached gets its minimum dwell; the scan reads as done only
  // when the real scan has finished and every stage has been on screen.
  function paceScan() {
    const scan = w.scan;
    if (scan.tick) return;
    scan.tick = setInterval(() => {
      // Its own scan only: a scan started again is a new object.
      if (!host.isConnected || w.scan !== scan || scan.state !== "scanning") {
        clearInterval(scan.tick);
        scan.tick = null;
        return;
      }
      if ((scan.shown || 0) < scanReal() && Date.now() - scan.since >= DWELL_MS) {
        scan.shown = (scan.shown || 0) + 1;
        scan.since = Date.now();
      }
      if (scan.realDone && scan.shown >= SCAN_STAGES.length) {
        clearInterval(scan.tick);
        scan.tick = null;
        scan.state = "done";
      }
      if (w.step === 3) paint();
    }, 150);
  }

  async function startScan() {
    w.scan = { state: "scanning", runs: [], error: "", shown: 0, since: Date.now() };
    const paths = w.sources.filter((s) => s.type !== "url").map((s) => s.path);
    if (!paths.length) {
      // Only URLs: nothing to scan. They are fetched when the run starts.
      w.scan.state = "done";
      return paint();
    }
    try {
      const target = w.split === "split" ? "each" : w.created;
      const splitPaths = w.split === "split" ? w.sources.flatMap((s) => (s.repoPaths && s.repoPaths.length ? s.repoPaths : [s.path])).filter((p) => !/^https:/.test(p)) : paths;
      const out = await post("/api/index", { path: splitPaths, store: target, review: "always", gitignore: w.gitignore, add_roots: target !== "each" });
      const errors = (out.runs || []).filter((r) => r.error);
      if (errors.length) throw new Error(errors.map((r) => `${r.path}: ${r.error}`).join("; "));
      w.scan.runs = (out.runs || []).map((r) => r.run).filter((x) => x !== undefined);
      w.scan.stores = [...new Set((out.runs || []).map((r) => r.store))];
      await load("runs", true);
    } catch (e) {
      w.scan = { state: "error", runs: [], error: e.message };
    }
    paint();
  }

  function reviewPanel() {
    const P = plan();
    const band = (r) => (r >= 70 ? "high" : r >= 30 ? "medium" : "low");
    const sorted = [...P.items].sort((a, b) => b.risk - a.risk);
    const inShow = (d) => w.decShow === "all" || (w.decShow === "open" ? !w.decisions[d.path] : !!w.decisions[d.path]);
    const shown = sorted.filter((d) => inShow(d) && (w.decRisk === "any" || band(d.risk) === w.decRisk));
    const selIds = P.items.filter((d) => w.decSel.has(d.path)).map((d) => d.path);
    const shownSel = shown.filter((d) => w.decSel.has(d.path)).length;
    const allSel = shown.length > 0 && shownSel === shown.length;
    const setDec = (ids, v) => {
      ids.forEach((id) => (v ? (w.decisions[id] = v) : delete w.decisions[id]));
      paint();
    };
    const credOnly = (ids) => ids.filter((id) => P.items.find((d) => d.path === id)?.class === "credential");
    const ACT = { out: "Keep out", redact: "Redact & index", in: "Index" };
    const RES = { out: ["Kept out", "grey"], redact: ["Redacted · indexed", "amber"], in: ["Will be indexed", "blue"] };
    // Recorded here, at once; the run applies them as its first phase. The
    // selection bar says so in the same words a store's Review tab uses.
    const bulk = (v) => () => {
      if (v && v !== "out" && credOnly(selIds).length) return toast("A credential file can only be kept out", true);
      w.bulk = { label: v ? ACT[v] : "Reset", total: selIds.length, done: selIds.length, over: true, local: true };
      setDec(selIds, v);
      w.decSel.clear();
      paint();
    };
    const openShown = shown.filter((d) => !w.decisions[d.path]);
    const cnt = (f) => sorted.filter(f).length;
    const kpis = el(
      "div",
      { class: "q4" },
      kpi("Will be indexed", n(P.embed + P.accepted), `${bytes(P.embedBytes)} of text${P.unchanged ? ` · ${n(P.unchanged)} unchanged` : ""}`),
      kpi("Your decision", P.undecided ? `${P.undecided} left` : "done", `${P.items.length} in the grey zone`, { warn: P.undecided > 0 }),
      kpi("Left out by rules", n(P.skipped), "ignored, generated, binary"),
      kpi("Credential files", n(P.credential), "never indexed, not even with OK"),
    );
    const parts = [kpis];
    if (P.items.length) {
      parts.push(
        el(
          "div",
          { class: "card" },
          el("div", { class: "card-h" }, el("span", { class: "card-t grow", text: "Needs your decision" }), meta(`${P.items.length - P.undecided} of ${P.items.length} decided · undecided files stay out`)),
          el(
            "div",
            { class: "filterbar" },
            seg(
              [
                ["all", "All", cnt(() => true)],
                ["open", "Undecided", cnt((d) => !w.decisions[d.path])],
                ["done", "Decided", cnt((d) => !!w.decisions[d.path])],
              ],
              w.decShow,
              (v) => ((w.decShow = v), paint()),
            ),
            seg(
              [
                ["any", "Any risk", cnt((d) => inShow(d))],
                ["high", "High", cnt((d) => inShow(d) && band(d.risk) === "high")],
                ["medium", "Medium", cnt((d) => inShow(d) && band(d.risk) === "medium")],
                ["low", "Low", cnt((d) => inShow(d) && band(d.risk) === "low")],
              ],
              w.decRisk,
              (v) => ((w.decRisk = v), paint()),
            ),
            el("span", { class: "spacer" }),
            openShown.length
              ? btn(
                  {
                    class: "btn sm",
                    onclick: () => {
                      openShown.forEach((d) => (w.decisions[d.path] = d.suggest || "out"));
                      w.decSel.clear();
                      w.bulk = { label: "Suggestions", total: openShown.length, done: openShown.length, over: true, local: true };
                      paint();
                    },
                  },
                  `Apply suggestions to ${openShown.length} undecided`,
                )
              : null,
          ),
          el(
            "div",
            { class: "gl-scroll" },
            el(
              "div",
              { class: "minw780" },
              el(
                "div",
                { class: "dec-grid head" },
                checkbox(allSel ? true : shownSel ? "mixed" : false, () => {
                  shown.forEach((d) => (allSel ? w.decSel.delete(d.path) : w.decSel.add(d.path)));
                  paint();
                }, "Select all shown"),
                el("span", { text: `FILE · ${shown.length} shown · highest risk first` }),
                el("span", { text: "RISK IF INDEXED" }),
                el("span", { text: "DECISION" }),
              ),
              selIds.length
                ? el(
                    "div",
                    { class: "selbar" },
                    el("span", { class: "what", text: `${plural(selIds.length, "file")} selected` }),
                    lnk("Clear", () => (w.decSel.clear(), paint())),
                    el("span", { class: "spacer" }),
                    btn({ class: "btn sm", onclick: bulk("out") }, "Keep out"),
                    btn({ class: "btn sm amber", onclick: bulk("redact") }, "Redact & index"),
                    btn({ class: "btn sm dark", onclick: bulk("in") }, "Index"),
                    selIds.some((id) => w.decisions[id]) ? lnk("Reset", bulk(null)) : null,
                  )
                : w.bulk
                  ? bulkLine(w.bulk, () => ((w.bulk = null), paint()))
                  : null,
              el(
                "div",
                { class: "dec-list", "data-scroll-keep": "dec" },
                !shown.length ? empty("No files match this filter.", "lg") : null,
                shown.map((d) => decisionRow(d, band, ACT, RES, setDec)),
              ),
            ),
          ),
          el("div", { class: "card-note", text: "Every decision is yours — one file or a selection — and is logged per file. An agent can never decide. Redact & index swaps each match for a typed placeholder before chunking; the value never reaches the index. An acceptance keeps a salted fingerprint of the match, never the value." }),
        ),
      );
    }
    parts.push(leftOutCard(P));
    return el("div", { class: "stack" }, parts);
  }

  function decisionRow(d, band, ACT, RES, setDec) {
    const v = w.decisions[d.path];
    const b = band(d.risk);
    const sel = w.decSel.has(d.path);
    const cred = d.class === "credential";
    return el(
      "div",
      { class: "dec-grid" },
      checkbox(sel, () => (sel ? w.decSel.delete(d.path) : w.decSel.add(d.path), paint())),
      el(
        "div",
        { class: "col gap4" },
        el("div", { class: "row" }, pathSpan(rel(d.path), "p", d.path), d.likely ? pill(d.likely, d.tone || "grey", { dot: false }) : null),
        el("span", { class: "why" }, el("b", { text: d.kind || classWord(d.class) }), ` · ${d.why || d.rule || ""}`),
        d.evidence || (d.matches || [])[0] ? el("span", { class: "evidence", text: d.evidence || maskedOf(d.matches[0]) }) : null,
      ),
      el("div", { class: "col gap4 risk" }, el("div", { class: "row base nowrap gap6" }, el("span", { class: `risk-pct ${b}`, text: `${d.risk}%` }), el("span", { class: "t-mono-sm", text: b })), bar(Math.max(3, d.risk), `h4 ${b === "high" ? "red" : b === "medium" ? "amber" : "green"}`)),
      el(
        "div",
        { class: "col gap4 acts" },
        v
          ? el("div", { class: "row" }, pill(RES[v][0], RES[v][1], { dot: false }), lnk("Undo", () => setDec([d.path], null)))
          : [
              el(
                "div",
                { class: "row gap6 nowrap" },
                btn({ class: "btn sm", onclick: () => setDec([d.path], "out") }, "Keep out"),
                btn({ class: "btn sm amber", disabled: cred ? true : null, onclick: () => setDec([d.path], "redact") }, "Redact & index"),
                btn({ class: "btn sm dark", disabled: cred ? true : null, onclick: () => setDec([d.path], "in") }, "Index"),
              ),
              el("span", { class: "muted t-xs", text: `Suggested · ${ACT[d.suggest || "out"]}` }),
            ],
      ),
    );
  }

  function leftOutCard(P) {
    const groups = [
      ["excluded", "Ignored by .gitignore and the rules", "What each repository already asks tools to skip, and what is outside the roots"],
      ["policy", "Build output, lockfiles and oversize files", "Generated, or over the size cap, so searching it only adds noise"],
      ["unindexable", "No text to read", "Binary, empty, or a format with no text layer"],
      ["credential", "Credential files", "Keys, tokens and .env files — never offered for indexing"],
    ]
      .map(([cls, label, desc]) => ({ cls, label, desc, count: P.notIndexed[cls] || (cls === "credential" ? P.credential : 0), paths: P.paths[cls] || [] }))
      .filter((g) => g.count);
    return el(
      "div",
      { class: "card" },
      el("div", { class: "card-h" }, el("span", { class: "card-t grow", text: "Left out automatically" }), meta("no action needed")),
      !groups.length ? empty(w.sources.every((s) => s.type !== "folder") ? "Nothing to leave out. Single files and URLs are read as they are." : "Nothing was left out.") : null,
      groups.map((g) => {
        const open = !!w.open[g.cls];
        return el(
          "div",
          { class: "line-row" },
          btn(
            { class: "group-row", "aria-expanded": String(open), onclick: () => ((w.open[g.cls] = !open), paint()) },
            icon(I.chevRight, 13, { w: 2 }),
            el("span", { class: "col" }, el("span", { class: "t-m", text: g.label }), el("span", { class: "muted t-xs", text: g.desc })),
            el("span", { class: "mono t-m ink2", text: n(g.count) }),
          ),
          open
            ? el(
                "div",
                { class: "group-paths" },
                g.paths.length ? g.paths.slice(0, 50).map((p) => pathSpan(rel(p), "", p)) : el("span", { text: "The list is written when the run ends; the count is the scan's." }),
                g.count > g.paths.length && g.paths.length ? el("span", { text: `…and ${n(g.count - g.paths.length)} more` }) : null,
              )
            : null,
        );
      }),
    );
  }

  /** A path under the source it came from, the way a person reads it. */
  function rel(path) {
    const roots = w.sources.map((s) => s.path).sort((a, b) => b.length - a.length);
    for (const r of roots) if (path.startsWith(r)) return path.slice(r.length).replace(/^[\\/]/, "") || baseName(path);
    return tilde(path);
  }

  // ---- step 4: index
  function step4() {
    const R = run();
    if (!R.length) return w.startSteps ? startingCard() : preRun();
    const live = R.find((x) => LIVE_RUN.has(x.status));
    if (live) return runningCard(live, R);
    return doneCard(R);
  }

  function preRun() {
    const P = plan();
    const files = P.embed + P.accepted;
    const urls = w.sources.filter((s) => s.type === "url").length;
    const lanes = (data.accel?.lanes || []).filter((l) => (l.status?.state || "") !== "unavailable");
    const fastest = lanes.filter((l) => l.enabled).sort((a, b) => (b.share || 0) - (a.share || 0))[0];
    const model = (data.privacy?.downloads || [])[0];
    const machine = data.runs?.limits?.machine;
    return el(
      "div",
      { class: "stack" },
      el(
        "div",
        { class: "auto-fit m150" },
        kpi("Files to index", n(files), P.accepted ? `includes ${P.accepted} you accepted` : urls ? `plus ${plural(urls, "URL")} fetched at the start` : "after review"),
        kpi("Text", bytes(P.embedBytes), P.unchanged ? `${n(P.unchanged)} unchanged, skipped by hash` : "read where it sits"),
        (() => {
          const e = estimate(P);
          const k = kpi("Estimate", e ? (e.ms < 60000 ? "under a minute" : spellAbout(e.ms)) : "—", e ? `on this machine${e.knownAnswer ? " (from the lane's known-answer speed)" : ""}` : "measured once the run starts");
          if (e) k.setAttribute("data-estimate-ms", String(Math.round(e.ms)));
          return k;
        })(),
      ),
      model && !model.cached
        ? el(
            "div",
            { class: "notice amber" },
            icon(I.download, 17, { w: 1.8 }),
            el("div", { class: "body" }, el("span", { class: "ttl", text: `One download first: the embedding model, ${bytes(model.bytes)}` }), el("span", { class: "sub", text: `From ${model.source}, once, cached in the model cache. It is the only thing semlith fetches on its own. Starting the run counts as your OK.` })),
          )
        : null,
      el(
        "div",
        { class: "card pad" },
        el("div", { class: "row base" }, el("span", { class: "card-t grow", text: "Run it on" }), meta(machine ? `${machine.logical_cores} cores · ${(machine.total_memory_mb / 1024).toFixed(0)} GiB` : "")),
        el(
          "div",
          { class: "auto-fit m180" },
          lanes.length
            ? lanes.map((l) =>
                btn(
                  {
                    class: `pick-card${l.enabled ? " on" : ""}`,
                    "aria-pressed": String(!!l.enabled),
                    "data-tip": l.enabled ? "On · applies to every run on this machine" : "Off · applies to every run on this machine",
                    onclick: () => laneToggle(l),
                  },
                  el("div", { class: "row nowrap" }, el("span", { class: "cb", "aria-checked": String(!!l.enabled) }), el("span", { class: "t", text: l.label || l.lane }), fastest && fastest.lane === l.lane && l.lane !== "cpu" ? el("span", { class: "rec", text: "fastest here" }) : null, l.experimental ? el("span", { class: "exp", text: "experimental" }) : null),
                  el("span", { class: "m12", text: l.device || l.variant || "" }),
                  el("span", { class: "d", text: l.download_bytes && !l.installed ? `needs a ${bytes(l.download_bytes)} download first` : l.lane === "cpu" ? "always available" : l.enabled ? laneWord(l.status) : "off" }),
                ),
              )
            : el("div", { class: "muted t-sm", text: "Reading the lanes…" }),
        ),
        el("span", { class: "muted t-xs", text: "Lanes are this machine's, set the same way in Settings › Performance. The run uses every lane that is on." }),
      ),
      el(
        "div",
        { class: "auto-fit m240 gap8" },
        toggleRow(w.watchAfter, "Keep watching after the run", "Saved files are re-indexed within a second.", (v) => ((w.watchAfter = v), paint())),
        toggleRow(w.record, "Record what agents retrieve", "A local ledger of queries and what they were sent. Never leaves the machine.", (v) => ((w.record = v), paint())),
      ),
    );
  }

  async function laneToggle(l) {
    const on = !l.enabled;
    if (on && l.download_bytes && !l.installed) {
      const ok = await ask({ title: `Turn ${l.label || l.lane} on?`, body: `It downloads the ${l.label || l.lane} pack first, ${bytes(l.download_bytes)}, once, into this machine's model cache.`, ok: "Download and turn on" });
      if (!ok) return;
    }
    // Drawn switched at once, so the estimate moves with the press; the
    // daemon's answer is read back over it.
    l.enabled = on;
    paint();
    await act(() => post("/api/accel", { lane: l.lane, action: on ? "on" : "off" }));
    await load("accel", true);
    paint();
  }

  /* Start (spec §3.1, W1) lands on the run view at once and says what each
   * request is doing; one that takes over two seconds says what it waits for.
   * The decisions are recorded only (`defer`): the run applies them as its
   * first phase, so nothing is embedded run-less before it starts. */
  async function startRun() {
    if (w.busy) return;
    w.busy = true;
    w.startSteps = [];
    // One display queue for Start's steps and then the run's, so the view
    // carries on from one to the other without starting its dwell again.
    w.startKey = `start:${Date.now()}`;
    const ticker = setInterval(() => host.isConnected && w.step === 4 && !run().length && paint(), 500);
    const step = async (label, waits, fn) => {
      const s = { label, waits, at: Date.now(), until: null };
      w.startSteps.push(s);
      paint();
      try {
        return await fn();
      } finally {
        s.until = Date.now();
      }
    };
    try {
      const runs = scanRuns();
      const stores = [...new Set(runs.map((r) => r.store).concat(w.created && w.split !== "split" ? [w.created] : []))];
      const P = plan();
      const by = {};
      let count = 0;
      for (const d of P.items) {
        const v = w.decisions[d.path];
        if (!v) continue;
        count += 1;
        (by[`${d.store}\n${v}`] = by[`${d.store}\n${v}`] || []).push(d.path);
      }
      if (count) {
        await step(`Recording ${plural(count, "decision")}`, "the daemon to record the decisions", async () => {
          for (const [key, files] of Object.entries(by)) {
            const [st, decision] = key.split("\n");
            for (let i = 0; i < files.length; i += BULK_BATCH) await post("/api/refused/decide", { store: st, files: files.slice(i, i + BULK_BATCH), decision, defer: true });
          }
        });
      }
      await step("Saving the store's settings", "the daemon to save the settings", async () => {
        for (const st of stores) await post("/api/store/settings", { store: st, watch: w.watchAfter, record: w.record, kind: w.kind }).catch(() => {});
      });
      const started = [];
      const held = runs.filter((x) => x.status === "review");
      if (held.length) {
        await step(`Starting ${held.length === 1 ? "the run" : plural(held.length, "run")}`, "the daemon to take the run", async () => {
          for (const r of held) {
            await post("/api/index/control", { store: r.store, run: r.id, action: "start" });
            started.push(r.id);
          }
        });
      }
      const urls = w.sources.filter((x) => x.type === "url");
      if (urls.length) {
        await step(`Adding ${plural(urls.length, "URL")}`, "the daemon to take the URLs", async () => {
          for (const s of urls) {
            const out = await post("/api/add", { url: s.path, store: w.created });
            for (const r of out.runs || []) started.push(r.run);
          }
        });
      }
      // A split run spent the named store on nothing: it is removed rather
      // than left empty beside the stores it was split into.
      if (w.split === "split" && w.created && !w.existing && !(w.scan.stores || []).includes(w.created)) {
        await post("/api/store/delete", { store: w.created }).catch(() => {});
      }
      await step("Reading the run back", "the daemon's list of runs", async () => {
        w.started = started.length ? started : runs.map((r) => r.id);
        await loadMany(["runs", "stores"], true);
      });
    } catch (e) {
      const last = w.startSteps[w.startSteps.length - 1];
      if (last) last.error = e.message;
      toast(e.message, true);
    }
    clearInterval(ticker);
    w.busy = false;
    paint();
  }

  /** What Start is doing before the run exists: its own requests, as steps. */
  function startingCard() {
    const steps = w.startSteps.map((s) => {
      const waited = (s.until || Date.now()) - s.at;
      return { label: s.label, at: s.at, until: s.until || (s.error ? s.at + waited : null), detail: s.error ? `Failed: ${s.error}` : !s.until && waited > 2000 ? `Waiting for ${s.waits} — ${Math.round(waited / 1000)} s so far` : "" };
    });
    const live = w.busy;
    const shown = shownSteps(w.startKey, steps);
    return el(
      "div",
      { class: "card run-card" },
      el(
        "div",
        { class: "card-b" },
        el("div", { class: "row" }, el("span", { class: "mono t-b", text: w.created || "" }), pill(live ? "starting" : "did not start", live ? "amber" : "red", { pulse: live })),
        timeline(shown, live),
        live ? null : btn({ class: "btn", onclick: () => ((w.startSteps = null), paint()) }, "Back to the plan"),
      ),
    );
  }

  // The steps the dwell queue lets through now; while it is behind the real
  // ones, the step is painted again shortly so each comes in on time.
  let dwellTimer = null;
  function shownSteps(key, steps) {
    const shown = dwell(key, steps);
    if (shown.behind && !dwellTimer) {
      dwellTimer = setTimeout(() => {
        dwellTimer = null;
        if (host.isConnected && w.step === 4) paint();
      }, 200);
    }
    return shown.steps;
  }

  function runningCard(r, all) {
    const p = runPct(r);
    const paused = r.status === "paused" || r.status === "pausing";
    // Start's own steps, then the run's: the decisions it applies, its place
    // in the queue, the lane starting, and on to the end.
    const pre = w.startSteps && w.startKey ? w.startSteps.map((s) => ({ label: s.label, at: s.at, until: s.until, detail: "" })) : [];
    // A run held for review began at its scan: what it did then is the scan
    // card's, and a step still going is drawn as starting after Start's.
    const from = pre.length ? pre[pre.length - 1].at : 0;
    const own = runSteps(r)
      .filter((s) => s.until == null || s.until >= from)
      .map((s) => (s.at < from ? { ...s, at: from } : s));
    const shown = shownSteps(w.startKey || `${r.store}:${r.id}:${r.submitted || ""}`, pre.concat(own));
    const log = el("div", { class: "log", "data-scroll-keep": "wzlog" });
    followLog(r, log);
    return el(
      "div",
      { class: "card run-card" },
      el(
        "div",
        { class: "card-b" },
        el(
          "div",
          { class: "row" },
          el("span", { class: "mono t-b", text: r.store }),
          pill(paused ? "paused" : r.status === "queued" ? `queued${r.position ? ` · ${r.position} in line` : ""}` : "indexing", paused || r.status === "queued" ? "amber" : "green", { pulse: !paused }),
          all.length > 1 ? meta(`${all.filter((x) => !LIVE_RUN.has(x.status)).length} of ${all.length} done`) : null,
          el("span", { class: "spacer" }),
          btn({ class: "btn", onclick: () => runControl(r, paused ? "resume" : "pause") }, paused ? "Resume" : "Pause"),
          btn({ class: "btn danger-soft", onclick: () => stopRun(r) }, "Stop…"),
        ),
        el(
          "div",
          { class: "row nowrap gap12" },
          (() => {
            const b = bar(p, "h8 accent grow");
            b.setAttribute("data-tip", `Index run · ${Math.floor(p)}%`);
            b.setAttribute("data-tip-rows", runRows(r));
            return b;
          })(),
          el("span", { class: "mono t-b", text: `${Math.floor(p)}%` }),
        ),
        timeline(shown, !paused),
      ),
      runStatsRow(r),
      log,
      el("div", { class: "lives" }, el("span", { class: "dot green" }), "The run lives in the daemon. Leave this page, close the tab — it keeps going and the header shows its progress."),
    );
  }

  function doneCard(R) {
    const failed = R.filter((r) => r.status === "failed" || r.status === "stopped");
    const name = w.created || R[0].store;
    const s = store(name);
    // The store's own totals once it reports them: a run counts only what it
    // embedded, not images it read or files it found unchanged.
    const files = Math.max(s?.files || 0, R.reduce((a, r) => a + (r.indexed || 0), 0));
    const chunks = Math.max(s?.chunks || 0, R.reduce((a, r) => a + (r.chunks || 0), 0));
    const tryInput = el("input", {
      value: w.tryQ,
      placeholder: "e.g. where does the watcher re-index a file",
      "data-keep": "wz-try",
      "aria-label": "Try a search",
      oninput: (e) => (w.tryQ = e.target.value),
      onkeydown: (e) => e.key === "Enter" && trySearch(),
    });
    return el(
      "div",
      { class: "stack" },
      failed.length
        ? el("div", { class: "notice red" }, el("div", { class: "body" }, el("span", { class: "ttl", text: `${failed.length} run${failed.length === 1 ? "" : "s"} did not finish` }), el("span", { class: "sub", text: failed.map((r) => `${r.store}: ${r.status}`).join(" · ") })))
        : el(
            "div",
            { class: "done-banner" },
            el("span", { class: "ic" }, icon(I.check, 17, { w: 2.6 })),
            el("div", { class: "col gap2 grow" }, el("span", { class: "t", text: `${name} is ready` }), el("span", { class: "l", text: `${plural(files, "file")} indexed · ${n(chunks)} chunks${s && s.disk ? ` · ${bytes(s.disk.total)}` : ""}${w.watchAfter ? " · watching for changes" : ""}` })),
          ),
      el(
        "div",
        { class: "card" },
        el(
          "div",
          { class: "card-b line-row" },
          el("div", { class: "row base" }, el("span", { class: "card-t grow", text: "Try it — ask what an agent would ask" }), meta("vector + keyword + graph")),
          el("div", { class: "row nowrap" }, el("div", { class: "box h38 focus grow" }, icon(I.searchSm, 15, { w: 1.8 }), tryInput), btn({ class: "btn md dark", onclick: trySearch }, "Search")),
        ),
        w.tried ? tryResults() : null,
      ),
    );
  }

  async function trySearch() {
    const q = w.tryQ.trim();
    if (!q) return;
    try {
      const out = await api(`/api/search?${new URLSearchParams({ query: q, store: w.created || "", k: "3" })}`);
      w.tried = out;
    } catch (e) {
      w.tried = { error: e.message };
    }
    paint();
  }

  function tryResults() {
    const t = w.tried;
    if (t.error) return errorBox(t.error);
    const hits = (t.hits || t.results || []).slice(0, 3);
    return el(
      "div",
      { class: "col" },
      !hits.length ? empty("Nothing matched. Try other words — or the store is still settling.") : null,
      hits.map((h) =>
        el(
          "div",
          { class: "try-row" },
          el("div", { class: "col gap2" }, el("span", { class: "mono t-m t-sm" }, `${rel(h.path)}:${h.start_line}-${h.end_line} `, el("span", { class: "muted", text: h.symbol ? `${h.kind || ""} ${h.symbol}`.trim() : "" })), el("span", { class: "txt-dim", text: (h.text || h.line || "").split("\n")[0] })),
          el("div", { class: "row gap4" }, listBadges(h.lists || [])),
        ),
      ),
      el("div", { class: "card-foot" }, `${hits.length} of ${n(t.total || hits.length)} shown${t.tokens ? ` · ${n(t.tokens)} tokens` : ""} · an agent gets the same answer over MCP`),
    );
  }

  // ---- step 5: connect
  function step5() {
    const clients = clientRows();
    const found = clients.filter((c) => c.found);
    if (!w.clientSel.size && !w.clientSelTouched) found.filter((c) => !c.registered).forEach((c) => w.clientSel.add(c.name));
    const sel = clients.filter((c) => w.clientSel.has(c.name));
    const done = w.reg === "done";
    return el(
      "div",
      { class: "stack" },
      el(
        "div",
        { class: "card" },
        el("div", { class: "card-h" }, el("span", { class: "card-t grow", text: "Clients on this machine" }), meta(`${found.length} of ${clients.length} found · ${sel.length} selected`)),
        el(
          "div",
          { class: "auto-fill m230" },
          clients.map((c) => {
            const on = w.clientSel.has(c.name);
            const registered = c.registered || (done && on && (w.regResult?.ok || []).includes(c.name));
            return btn(
              {
                class: `client-pick${on ? " on" : ""}`,
                disabled: !c.found || done || c.registered ? true : null,
                onclick: () => {
                  w.clientSelTouched = true;
                  on ? w.clientSel.delete(c.name) : w.clientSel.add(c.name);
                  paint();
                },
              },
              el("span", { class: "cb", "aria-checked": String(on || c.registered) }),
              el("span", { class: "col grow min0" }, el("span", { class: "nm", text: c.name }), el("span", { class: "muted t-xs anywhere", text: c.found ? c.how : "not found on this machine" })),
              registered ? pill("registered", "green", { dot: false }) : c.found ? el("span", { class: "t-mono-sm", text: "found" }) : el("span", { class: "t-mono-sm", text: "—" }),
            );
          }),
        ),
      ),
      el(
        "div",
        { class: "card" },
        el("div", { class: "card-h" }, el("span", { class: "card-t grow", text: "What semlith will write" }), meta("each file backed up beside itself first")),
        sel.length
          ? el(
              "div",
              { class: "tw" },
              el(
                "table",
                { "aria-label": "What semlith will write" },
                el("thead", {}, el("tr", {}, el("th", { text: "Client" }), el("th", { text: "How" }), el("th", { text: "Result" }))),
                el(
                  "tbody",
                  {},
                  sel.map((c) => {
                    const okNow = (w.regResult?.ok || []).includes(c.name);
                    const bad = (w.regResult?.failed || []).find((f) => f.client === c.name);
                    return el(
                      "tr",
                      {},
                      el("td", { class: "t-m nowrap", text: c.name }),
                      el("td", { class: "ms anywhere", text: c.write }),
                      el("td", { class: "nowrap" }, bad ? pill(bad.error || "failed", "red", { dot: false }) : okNow ? pill("registered · backup saved", "green", { dot: false }) : w.reg === "busy" ? pill("writing…", "blue", { dot: false }) : pill("will add semlith", "grey", { dot: false })),
                    );
                  }),
                ),
              ),
            )
          : empty("Pick at least one client above, or skip — the Agents page does this any time."),
        el("div", { class: "card-foot sans" }, lnk(w.manual ? "Hide the manual setup" : "Set a client up by hand instead", () => ((w.manual = !w.manual), paint())), el("span", { class: "spacer" }), el("span", { class: "t-mono-sm", text: data.agents?.endpoint?.url || "" })),
        w.manual ? codeBlock(mcpJson(), { cls: "flat", word: "Config copied" }) : null,
      ),
    );
  }

  async function register() {
    const names = [...w.clientSel];
    if (!names.length) return;
    w.reg = "busy";
    paint();
    try {
      const out = await post("/api/agents/register", { clients: names, action: "register", confirm: true });
      const results = out.results || [];
      w.regResult = {
        ok: results.filter((r) => r.ok).map((r) => r.client),
        failed: results.filter((r) => !r.ok),
      };
      w.reg = "done";
      toast(`Registered ${plural(w.regResult.ok.length, "client")}`);
      load("agents", true).then(paint);
    } catch (e) {
      w.reg = "idle";
      toast(e.message, true);
    }
    paint();
  }

  // ---- summary rail and footer
  let railNode = null;
  function summaryRail() {
    railNode = el("div", { class: "wz-rail" });
    paintRail();
    return railNode;
  }

  function paintRail() {
    if (!railNode) return;
    const P = plan();
    const R = run();
    const live = R.find((x) => LIVE_RUN.has(x.status));
    const step = w.step;
    const nm = w.name.trim();
    const rowsList = [
      ["NAME", nm || "not named yet", !!w.created, !nm, true],
      ["HOLDS", { code: "Code", docs: "Docs & notes", both: "Code and docs" }[w.kind], !!w.created],
      ["SOURCES", sourcesLine(), w.sources.length > 0 && step > 2, !w.sources.length, true],
      ["REVIEW", step < 3 || w.scan.state !== "done" ? "after the scan" : P.undecided ? `${P.undecided} undecided — they stay out` : `${P.accepted ? `${P.accepted} accepted · ` : ""}all decided`, step > 3, step < 3],
      // Nothing about the index is known until the scan is done.
      w.scan.state !== "done"
        ? ["INDEX", step < 3 ? "not started" : "after the scan", false, true]
        : [
            "INDEX",
            R.length ? (live ? `${Math.floor(runPct(live))}% · ${live.status}` : `${n(R.reduce((a, r) => a + (r.indexed || 0), 0))} files · ready`) : step >= 4 ? `${n(P.embed + P.accepted)} files${estimate(P) ? ` · ${spellAbout(Math.max(60000, estimate(P).ms))}` : ""}` : "not started",
            R.length && !live,
            !R.length && step < 4,
          ],
    ];
    if (w.onboarding) rowsList.push(["AGENTS", w.reg === "done" ? `${(w.regResult?.ok || []).length} connected` : "not yet", w.reg === "done", w.reg !== "done"]);
    const tips = {
      1: "A store is one index with one model. Make separate stores for things you'd search separately — a client's code and your own notes, say.",
      2: "Drop a parent folder and semlith looks for the repositories inside it. Nothing is copied or moved: files are read where they sit.",
      3: "Credential files — keys, tokens, .env — are never offered, even with your OK. What you see here is only the grey zone.",
      4: "The run lives in the daemon, not this page. Close the tab or let the laptop sleep — it picks up where it was.",
      5: "Each config file is backed up beside itself before it is written, and a file that doesn't parse is left untouched.",
    };
    fill(
      railNode,
      el(
        "div",
        { class: "card" },
        el("div", { class: "card-h" }, el("span", { class: "card-t", text: "Your store" })),
        rowsList.map(([k, v, ok, dim, mono]) => el("div", { class: "sum-row" }, el("span", { class: "k", text: k }), el("span", { class: `v${dim ? " dim" : ""}${mono ? "" : " sans"}`, text: v }), ok ? icon(I.check, 13, { w: 2.6 }) : el("span"))),
      ),
      el("div", { class: "blue-box" }, el("span", { class: "eyebrow", text: "Good to know" }), el("span", { class: "txt", text: tips[w.step] })),
    );
  }

  let nextFn = null;
  async function next() {
    if (nextFn) await nextFn();
  }

  function paintFoot() {
    const step = w.step;
    const R = run();
    const live = R.find((x) => LIVE_RUN.has(x.status));
    const ns = nameState();
    let label = "Continue";
    let on = true;
    let hint = "";
    let sec = null;
    nextFn = null;
    if (step === 1) {
      label = w.created ? (w.created === ns.nm ? "Continue" : "Rename and continue") : "Create store";
      on = ns.ok && !w.busy;
      hint = ns.ok ? `Creates an empty store at ~/.semlith/stores/${ns.nm}` : "Pick a name to continue";
      nextFn = createOrRename;
    } else if (step === 2) {
      const k = w.sources.length;
      label = `Scan ${k || ""} ${k === 1 ? "source" : "sources"}`.replace("  ", " ");
      on = k > 0;
      hint = k ? "Scanning reads names and hashes only — nothing is embedded yet" : "Add at least one folder, file or URL";
      nextFn = async () => {
        w.scan = { state: "idle", runs: [], error: "" };
        w.decisions = {};
        w.bulk = null;
        w.startSteps = null;
        setStep(3);
      };
    } else if (step === 3) {
      const P = plan();
      label = "Continue to index";
      on = w.scan.state === "done";
      hint = w.scan.state !== "done" ? "Scanning…" : P.undecided ? `${P.undecided} undecided — they stay out, and you can decide later on the store's Review tab` : "Decisions can be changed later on the store's Review tab";
      nextFn = async () => setStep(4);
    } else if (step === 4) {
      if (!R.length) {
        const P = plan();
        label = w.busy ? "Starting…" : "Start indexing";
        on = !w.busy;
        const e = estimateText(estimate(P));
        hint = w.busy ? "Starting — each step is on the left" : `${plural(P.embed + P.accepted, "file")}${e ? ` · ${e}` : ""}`;
        nextFn = startRun;
      } else if (live) {
        if (w.onboarding) {
          label = "Next: connect agents";
          nextFn = async () => setStep(5);
          hint = "Indexing carries on in the background";
        } else {
          label = "Run in background";
          nextFn = async () => {
            const name = w.created || live.store;
            state.wz = null;
            go("store", name, "runs");
          };
          hint = "Watch it from the store's Runs tab or the header";
        }
      } else {
        if (w.onboarding) {
          label = "Next: connect agents";
          nextFn = async () => setStep(5);
        } else {
          label = "Open store";
          nextFn = async () => {
            const name = w.created || R[0].store;
            state.wz = null;
            go("store", name);
          };
        }
        hint = "Ready — agents can search it now";
      }
    } else if (step === 5) {
      const done = w.reg === "done";
      const finish = async () => {
        state.wz = null;
        await load("stores", true);
        go("home");
      };
      if (done) {
        label = "Go to Home";
        nextFn = finish;
        hint = `${plural((w.regResult?.ok || []).length, "client")} registered — restart them to pick semlith up`;
      } else {
        sec = ["Skip for now", finish];
        const k = w.clientSel.size;
        label = w.reg === "busy" ? "Registering…" : `Register ${k} ${k === 1 ? "client" : "clients"}`;
        on = k > 0 && w.reg !== "busy";
        hint = "Nothing is written until you press Register";
        nextFn = register;
      }
    }
    const backOn = step > 1 && !(step === 4 && live) && !(step === 2 && w.existing);
    fill(
      foot,
      el(
        "div",
        { class: "wz-foot-in" },
        btn({ class: "btn md", disabled: backOn ? null : true, hidden: step === 1 ? true : null, onclick: () => backOn && setStep(step - 1) }, icon(I.back, 14, { w: 1.8 }), "Back"),
        el("span", { class: "hint hide-sm", text: hint }),
        sec ? btn({ class: "btn md", onclick: sec[1] }, sec[0]) : null,
        btn({ class: "btn md primary", disabled: on ? null : true, onclick: next }, label, icon(I.arrow, 14, { w: 2 })),
      ),
    );
    paintRail();
  }

  async function createOrRename() {
    const ns = nameState();
    if (!ns.ok || w.busy) return;
    w.busy = true;
    paintFoot();
    try {
      if (!w.created) {
        await post("/api/store/create", { name: ns.nm, kind: w.kind });
        toast(`Created ${ns.nm} — empty until you add sources`);
      } else if (w.created !== ns.nm) {
        await post("/api/store/settings", { store: w.created, rename: ns.nm, kind: w.kind });
        toast(`Renamed to ${ns.nm}`);
      } else {
        await post("/api/store/settings", { store: w.created, kind: w.kind }).catch(() => {});
      }
      w.created = ns.nm;
      await load("stores", true);
      w.busy = false;
      setStep(2);
    } catch (e) {
      w.busy = false;
      toast(e.message, true);
      paintFoot();
    }
  }

  // Live: the runs the wizard started or scanned move with the poll.
  const offRuns = onRunsChange(() => {
    if (!host.isConnected) return offRuns();
    const scans = scanRuns();
    if (w.scan.state === "scanning" && scans.length && scans.every((r) => !["running", "queued", "scanning"].includes(r.status))) {
      // A failure shows at once; a success waits for the card to have shown
      // every stage (paceScan), unless it already has.
      if (scans.some((r) => r.status === "failed")) {
        w.scan.state = "error";
        w.scan.error = "The scan failed. Its log is on the store's Runs tab.";
      } else {
        w.scan.realDone = true;
        if ((w.scan.shown ?? SCAN_STAGES.length) >= SCAN_STAGES.length) w.scan.state = "done";
      }
    }
    // When the started runs end, the store's own totals are read once more,
    // so the done card counts what the store holds now.
    const started = run();
    const ended = started.length && started.every((r) => !LIVE_RUN.has(r.status));
    if (ended && !w.endedRead) {
      w.endedRead = true;
      load("stores", true).then(() => w.step === 4 && paint());
    }
    if (w.step === 3 || w.step === 4) paint();
    else paintRail();
  });

  paint();
  return host;
}

/* Poll a run's log while its card is on screen, from its own cursor. */
function followLog(r, logNode) {
  const key = `${r.store}:${r.id}`;
  const seen = (followLog.cursors[key] = followLog.cursors[key] || { after: r.log_from ?? null, lines: [] });
  const draw = () => {
    fill(
      logNode,
      seen.lines.slice(-120).map((ev) => {
        const [a, b, c, tone] = logParts(ev);
        return el("div", { class: "ln" }, el("span", { class: "a", text: a }), el("span", { class: `b ${tone}`, text: b }), el("span", { class: "c", text: c }));
      }),
    );
    logNode.scrollTop = logNode.scrollHeight;
  };
  draw();
  const tick = async () => {
    if (!logNode.isConnected) return;
    try {
      const out = await api(`/api/index/log?${new URLSearchParams({ store: r.store, run: String(r.id), ...(seen.after != null ? { after: String(seen.after) } : {}) })}`);
      if (out.lines && out.lines.length) {
        seen.lines.push(...out.lines);
        if (seen.lines.length > 400) seen.lines.splice(0, seen.lines.length - 400);
        seen.after = out.cursor;
        draw();
      }
    } catch (_) {
      /* the next tick tries again */
    }
    const live = (data.runs?.runs || []).find((x) => x.id === r.id);
    if (live && LIVE_RUN.has(live.status)) setTimeout(tick, 1000);
  };
  // After the caller has put the box on the page: called while the card is
  // still being built, the box is not connected yet, and a first tick run now
  // would stop at once and the log would stay empty for the whole run.
  setTimeout(tick, 0);
}
followLog.cursors = {};

function logParts(ev) {
  const when = ev.at ? clock(msOf(ev.at) / 1000) : "";
  if (ev.event === "file") {
    const outcome = ev.outcome || "read";
    const tone = /refus|skip|fail/.test(outcome) ? "bad" : /embed|index|image/.test(outcome) ? "ok" : /unchanged|queued/.test(outcome) ? "" : "info";
    return [`${n(ev.scanned)}/${n(ev.total)}`, outcome, ev.why ? `${ev.path} — ${ev.why}` : ev.path, tone];
  }
  if (ev.event === "phase") return [when, phaseLabel(ev.phase, ev.detail, ev.lane), ev.detail || "", "info"];
  const text = {
    submitted: ev.ahead ? `waiting — ${plural(ev.ahead, "run")} ahead of this one` : "submitted",
    queued: ev.ahead ? `waiting for ${ev.store}'s writer — ${plural(ev.ahead, "job")} ahead` : "waiting for the store's writer",
    started: "walking the tree and hashing what it finds",
    slice: `${n(ev.remaining)} paths left; giving the watcher a turn`,
    paused: "held between files — the writer is still this run's",
    resumed: "carrying on",
    error: ev.error || "failed",
    done: ev.dequeued ? "removed from the queue before it started; nothing was indexed" : ev.stopped ? "stopped — everything this run embedded was undone" : `${n(ev.indexed)} indexed, ${n(ev.unchanged)} unchanged, ${n(ev.skipped)} skipped, ${n(ev.removed)} removed, ${n(ev.chunks)} chunks`,
  }[ev.event];
  return [when, ev.event || "", text || ev.text || "", ev.event === "error" ? "bad" : ev.event === "done" ? "ok" : "info"];
}

/** Chunks embedded, of those the run expects once a run-truth daemon says. */
function chunksOf(r) {
  return r.expected_chunks ? `${n(r.chunks || 0)} / ${n(r.expected_chunks)}` : n(r.chunks || 0);
}

function laneRatesText(r) {
  return Object.entries(r.lane_rates || {})
    .filter(([, v]) => v > 0)
    .map(([k, v]) => `${laneName(k)} ${perSecond(v)}/s`)
    .join(" · ");
}

function runRows(r) {
  return rows([
    ["files", r.total ? `${n(r.scanned)} / ${n(r.total)}` : ""],
    ["chunks", chunksOf(r)],
    ["images", r.images_total ? `${n(r.images || 0)} / ${n(r.images_total)}` : ""],
    ["rate", r.rate != null ? `${perSecond(r.rate)} chunks/s` : ""],
    ["time left", r.status === "running" ? runLeftText(r) : ""],
  ]);
}

function runStatsRow(r) {
  const paused = r.status === "paused" || r.status === "pausing";
  const lanes = laneRatesText(r);
  const tiles = [
    ["FILES READ", r.total ? `${n(r.scanned)} / ${n(r.total)}` : "counting…", r.bytes_total ? `${bytes(r.bytes || 0)} of ${bytes(r.bytes_total)} read` : "Files read and hashed; embedding follows, counted in chunks"],
    ["CHUNKS", chunksOf(r), r.expected_chunks ? "Embedded of those expected; the total firms up as each file is chunked" : null, r.backlog ? `${n(r.backlog)} waiting for a lane` : null],
    r.images_total ? ["IMAGES", `${n(r.images || 0)} / ${n(r.images_total)}`, "Images embedded with CLIP"] : null,
    ["RATE", paused ? "paused" : r.rate != null ? `${perSecond(r.rate)} chunks/s` : "—", lanes, paused ? null : lanes],
    ["TIME LEFT", paused ? "—" : r.status === "running" ? runLeftText(r) : r.status, runRangeTip(r), r.elapsed_ms ? `${spellTook(r.elapsed_ms)} elapsed` : null],
  ].filter(Boolean);
  return el(
    "div",
    { class: `run-stats${tiles.length > 4 ? " five" : ""}` },
    tiles.map(([k, v, t, s]) => el("div", { "data-tip": t || null }, el("span", { class: "eyebrow sm wide", text: k }), el("span", { class: "v", text: v }), s ? el("span", { class: "s", text: s }) : null)),
  );
}

const LANE_NAMES = { cpu: "CPU", ane: "Neural Engine", gpu: "GPU", cuda: "CUDA", trt: "TensorRT for RTX", openvino: "OpenVINO", llama: "llama.cpp", worker: "Worker" };
const laneName = (k) => LANE_NAMES[k] || k;

async function runControl(r, action) {
  // Pressing Pause says "pausing" at once: the request is on its way, and a
  // poll that lands meanwhile must not be the first to speak.
  if (action === "pause") {
    PAUSING.set(`${r.store}:${r.id}`, Date.now() + 5000);
    const now = (data.runs?.runs || []).find((x) => x.id === r.id && x.store === r.store);
    if (now && now.status === "running") {
      now.status = "pausing";
      repaint();
    }
  }
  const out = await act(() => post("/api/index/control", { store: r.store, run: r.id, action }), action === "pause" ? "Pausing at the next batch" : action === "resume" ? "Resumed" : null);
  // The route's own word first ("pausing", not yet "paused"), then the runs.
  const mine = out && out.state && (data.runs?.runs || []).find((x) => x.id === r.id && x.store === r.store);
  if (mine) {
    mine.status = out.state;
    repaint();
  }
  if (action !== "pause") PAUSING.delete(`${r.store}:${r.id}`);
  await load("runs", true);
  for (const fn of [...runsListeners]) fn();
  paintChrome();
}

async function stopRun(r) {
  const fresh = !(r.files_before || r.chunks_before);
  const del = el("input", { type: "checkbox", id: "stop-delete" });
  if (fresh) del.checked = true;
  const ok = await ask({
    title: `Stop ${r.store}'s index run?`,
    body: "What it has embedded so far is undone, so the store is left exactly as it was before the run. The files on disk are untouched.",
    extra: fresh ? el("label", { class: "modal opt", for: "stop-delete" }, del, el("span", { text: "Also delete the store — it held nothing before this run" })) : null,
    ok: "Stop and undo",
    cancel: "Keep running",
    danger: true,
  });
  if (!ok) return;
  await act(() => post("/api/index/control", { store: r.store, run: r.id, action: r.status === "queued" ? "dequeue" : "stop", delete: fresh && del.checked }), "Stopping — undoing what it embedded");
  await loadMany(["runs", "stores"], true);
  for (const fn of [...runsListeners]) fn();
  paintChrome();
}


function classWord(cls) {
  return { content: "Secret-shaped value", policy: "Policy", credential: "Credential file", unindexable: "No text", excluded: "Excluded", dummy: "Test dummy" }[cls] || cls || "";
}

function maskedOf(match) {
  if (!match) return "";
  return match.masked || match.mask || match.preview || match.line || "";
}

/* What a file would cost if indexed, as a percentage. The daemon sends `risk`
 * from its own scan (0.35.0); before that the scan's confidence stands in. */
function riskOf(item) {
  if (typeof item.risk === "number") return Math.round(item.risk);
  if (item.class === "credential") return 99;
  if (typeof item.confidence === "number") return item.confidence;
  return item.class === "policy" ? 8 : 40;
}

/** The fusion lists that found a hit, as the design's small badges. */
function listBadges(lists) {
  const tone = { vector: "blue", keyword: "amber", graph: "green", definition: "outline" };
  return (lists || []).map((l) => el("span", { class: `badge ${tone[l] || ""}`, text: l }));
}

/** The MCP stanza a client written by hand carries. */
function mcpJson() {
  const url = data.agents?.endpoint?.url || `${location.protocol}//${location.host}/mcp`;
  const env = data.agents?.key_env || "SEMLITH_AGENT_KEY";
  return `{\n  "mcpServers": {\n    "semlith": {\n      "type": "http",\n      "url": "${url}",\n      "headers": { "Authorization": "Bearer \${${env}}" }\n    }\n  }\n}`;
}

/** Every documented client with what this machine says about it. */
// The command's name and verb, without the arguments that carry paths or JSON.
function cmdHead(text) {
  const words = [];
  for (const w of String(text).split(/\s+/)) {
    if (words.length === 3 || /[{"'\/]/.test(w)) break;
    words.push(w);
  }
  return words.join(" ");
}

function clientRows() {
  const reports = data.agents?.doctor || [];
  return (data.agents?.clients || []).map((c) => {
    const r = reports.find((x) => x.name === c.name) || {};
    const register = (c.stanzas || []).find((s) => s.register);
    const file = (c.stanzas || []).find((s) => s.path);
    const found = !!(r.present || (r.files || []).some((f) => f.exists));
    return {
      name: c.name,
      group: c.group,
      found,
      registered: !!r.registered,
      report: r,
      client: c,
      how: `${c.group} · ${register ? cmdHead(register.text) : file ? file.path : "by hand"}`,
      write: register ? register.text.replace(/\s+/g, " ") : file ? file.path : "set up by hand",
    };
  });
}

// ---------------------------------------------------------------- store state

/** A store's state in the words and tone of the design's pill. */
function storeState(s) {
  if (s.missing) return { state: "missing", tone: "red", tip: `Its directory is gone: ${s.dir}` };
  if (s.unreadable) return { state: "unreadable", tone: "red", tip: "Its database could not be read" };
  if (s.unopened) return { state: "not open", tone: "grey", tip: s.unopened };
  const r = activeRun(s.name);
  if (r) {
    const p = Math.floor(runPct(r));
    if (r.status === "review") return { state: "waiting for review", tone: "amber" };
    if (r.status === "queued") return { state: "queued", tone: "blue" };
    if (r.status === "paused" || r.status === "pausing") return { state: `paused ${p}%`, tone: "amber" };
    return { state: `${r.kind === "compact" ? "compacting" : "indexing"} ${p}%`, tone: "blue", pulse: true };
  }
  const review = reviewCount(s.name);
  // Before "not indexed": once a deleted root's files are pruned the store
  // has none, and a missing folder is what the reader can act on.
  if ((s.roots || []).some((r2) => !r2.present)) return { state: "root missing", tone: "red" };
  if (!s.files && !(s.roots || []).length) return { state: "empty", tone: "grey" };
  if (!s.files) return { state: "not indexed", tone: "grey" };
  if (review) return { state: `${review} to review`, tone: "amber" };
  if (s.watching === false && s.watch !== false && s.stopped_because) return { state: "not watching", tone: "amber", tip: s.stopped_because };
  return { state: "fresh", tone: "green" };
}

function statePill(s) {
  const st = storeState(s);
  return pill(st.state, st.tone, { pulse: st.pulse, tip: st.tip });
}

function reviewCount(name) {
  return ((data.refused?.stores || []).find((x) => x.store === name) || {}).review || 0;
}

function kindOf(s) {
  return { code: "code", docs: "docs", both: "code + docs", mixed: "code + docs" }[s.kind || "both"] || "code + docs";
}

// The Stores page's per-store saving, never without its coverage and tier.
// Nothing to read again when every folder a store reads from is gone.
function rootsGone(s) {
  const roots = s.roots || [];
  return roots.length > 0 && roots.every((r) => r.present === false);
}

function savedLine(s) {
  if (!s.savings || !s.savings.total) return "nothing asked yet";
  return `${short(s.savings.net_tokens)} fewer · coverage ${s.savings.coverage}% · ${s.savings.tier}`;
}

// Where a store reads from, on one line: the first root, shortened, and how
// many more. Every root in full is in rootsAll for the tooltip.
function rootsLine(s) {
  const roots = (s.roots || []).map((r) => r.path);
  if (!roots.length) return "no sources yet";
  return `${shortPath(roots[0], 48)}${roots.length > 1 ? ` + ${roots.length - 1} more` : ""}`;
}

function rootsAll(s) {
  return (s.roots || []).map((r) => tilde(r.path)).join("\n") || null;
}

function diskOf(s) {
  return s.disk ? s.disk.total : 0;
}

function reclaimable(s) {
  return s.disk && s.disk.reclaimable > 0 && (s.disk.dead_percent || 0) >= 5 ? s.disk.reclaimable : 0;
}

// ---------------------------------------------------------------- store actions

async function reindexStore(name, files) {
  const s = store(name);
  if (!s) return;
  const paths = files && files.length ? files : (s.roots || []).map((r) => r.path);
  if (!paths.length) return openWizard({ store: name });
  const out = await act(() => post("/api/index", { store: name, path: files ? undefined : paths, files: files || undefined }), files ? `Queued ${plural(files.length, "file")} to re-index in ${name}` : `Re-indexing ${name} — unchanged files are skipped by hash`);
  if (out) await loadMany(["runs", "stores"], true), paintChrome();
}

async function compactStores(names) {
  const list = names.filter(Boolean);
  if (!list.length) return;
  const free = list.reduce((a, nm) => a + reclaimable(store(nm) || {}), 0);
  const ok = await ask({
    title: list.length === 1 ? `Compact ${list[0]}?` : `Compact ${list.length} stores?`,
    body: `Rewrites the vectors without the deleted chunks and vacuums the database${free ? `, giving back about ${bytes(free)}` : ""}. Searches keep working while it runs; nothing that answers today changes.`,
    ok: "Compact",
  });
  if (!ok) return;
  for (const nm of list) await act(() => post("/api/store/compact", { store: nm }));
  toast(list.length === 1 ? `Compacting ${list[0]}` : `Compacting ${list.length} stores`);
  await load("runs", true);
  paintChrome();
}

async function forgetStore(name) {
  const s = store(name);
  const ok = await ask({
    title: `Forget ${name}?`,
    body: `Its index, vectors and ledger rows are deleted. The ${s && s.files ? plural(s.files, "file") : "files"} it read stay exactly where they are.`,
    ok: "Forget store",
    cancel: "Keep it",
    danger: true,
  });
  if (!ok) return false;
  const out = await act(() => post("/api/store/delete", { store: name }), `Forgot ${name}`);
  if (!out) return false;
  await load("stores", true);
  return true;
}

function searchStore(name) {
  state.pending.searchStore = name;
  go("search");
}

function storeMenu(s) {
  return () => [
    { label: "Open", onclick: () => go("store", s.name) },
    { label: "Add sources", onclick: () => openWizard({ store: s.name }) },
    { label: "Search it", onclick: () => searchStore(s.name) },
    { label: "Re-index", disabled: !!activeRun(s.name) || rootsGone(s), onclick: () => reindexStore(s.name) },
    reclaimable(s) ? { label: "Compact", hint: bytes(reclaimable(s)), onclick: () => compactStores([s.name]) } : null,
    { label: "Forget…", tone: "red", onclick: () => forgetStore(s.name) },
  ];
}

// ---------------------------------------------------------------------- home

VIEWS.home = {
  needs: () => ["stores", "runs", "refused", "ledger", "agents"],
  live: ["stores", "runs", "refused", "ledger", "agents"],
  render() {
    const stores = liveStores();
    const all = data.stores?.stores || [];
    const ledger = data.ledger || {};
    const agents = data.agents || {};
    const files = stores.reduce((a, s) => a + (s.files || 0), 0);
    const chunks = stores.reduce((a, s) => a + (s.chunks || 0), 0);
    const disk = stores.reduce((a, s) => a + diskOf(s), 0);
    const reviewing = stores.filter((s) => reviewCount(s.name));
    const running = (data.runs?.runs || []).filter((r) => LIVE_RUN.has(r.status));
    const registered = registeredClients();
    const found = (agents.doctor || []).filter((c) => c.present || (c.files || []).some((f) => f.exists));
    const prefs = readPrefs();

    const items = [
      ["Create a store", "Name it, add sources, review, index", stores.length > 0, () => openWizard({})],
      ["Connect an agent", "Register the clients found here", registered.length > 0, () => go("agents", "add")],
      ["Run a first search", "See what an agent will be sent", (ledger.queries || 0) > 0 || state.recents.length > 0, () => go("search")],
      ["Check what leaves the machine", "One minute on the Privacy page", !!prefs.privacySeen, () => go("privacy")],
    ];
    const nDone = items.filter((i) => i[2]).length;
    const checklist =
      !prefs.checklistDismissed && nDone < 4
        ? el(
            "div",
            { class: "card" },
            el(
              "div",
              { class: "card-h" },
              el("span", { class: "card-t", text: "Getting started" }),
              meta(`${nDone} of 4 done`),
              (() => {
                const b = bar(nDone * 25, "h5 green w120");
                b.setAttribute("data-tip", `${nDone} of 4 done`);
                b.setAttribute("data-tip-rows", items.map((i) => `${i[0]}::${i[2] ? "done" : "to do"}`).join("||"));
                return b;
              })(),
              el("span", { class: "spacer" }),
              lnk("Dismiss", () => (savePrefs({ checklistDismissed: true }), repaint()), "muted"),
            ),
            el(
              "div",
              { class: "check4" },
              items.map(([t, d, done, fn], i) =>
                btn(
                  { onclick: fn },
                  el("span", { class: `cdot${done ? " done" : i === items.findIndex((x) => !x[2]) ? " next" : ""}` }, done ? icon(I.check, 10, { w: 3.2 }) : null),
                  el("span", { class: "col gap2" }, el("span", { class: `ctitle${done ? " done" : ""}`, text: t }), el("span", { class: "muted t-xs pretty", text: d })),
                ),
              ),
            ),
          )
        : null;

    const lastQuery = (ledger.rows || [])[0];
    const kpis = el(
      "div",
      { class: "q4" },
      kpi("Stores", String(stores.length), running.length ? `${plural(running.length, "run")} going now` : reviewing.length ? `${plural(reviewing.length, "store")} ${reviewing.length === 1 ? "needs" : "need"} a review` : stores.length ? "all fresh and watched" : "none yet", { onclick: () => go("stores") }),
      kpi("Files indexed", n(files), `${n(chunks)} chunks`, { onclick: () => go("stores") }),
      kpi("Agents connected", String(connectedCount()), connectedCount() ? (lastQuery ? `${lastQuery.client} asked ${ago(lastQuery.at)}` : "waiting for a first query") : registered.length ? `${plural(registered.length, "client")} registered · none talking now` : `${plural(found.length, "client")} found on this machine`, { onclick: () => go("agents") }),
      kpi("Fewer tokens", ledger.ratio ? `${ledger.ratio.toFixed(1)}×` : "—", ledger.ratio ? `than reading those files whole · coverage ${ledger.coverage}% · ${ledger.tier}` : "counted once an agent asks something", { onclick: () => go("ledger") }),
    );

    const storesCard = el(
      "div",
      { class: "card" },
      el("div", { class: "card-h" }, el("span", { class: "card-t grow", text: "Stores" }), lnk("+ New store", () => openWizard({})), el("span", { class: "vrule" }), lnk("All stores →", () => go("stores"))),
      el(
        "div",
        { class: "gl-scroll" },
        el(
          "div",
          { class: "minw560" },
          el("div", { class: "gl-head gl-home-stores" }, el("span", { text: "STORE" }), el("span", { text: "STATE" }), el("span", { class: "right", text: "FILES" }), el("span", { text: "SAVED" }), el("span", { class: "right", text: "WRITTEN" })),
          all.length
            ? all.slice(0, 8).map((s) =>
                btn(
                  { class: "gl-row gl-home-stores", onclick: () => go("store", s.name) },
                  el("span", { class: "cellname" }, el("span", { class: "a", text: s.name }), el("span", { class: "b ell-start", "data-tip": rootsAll(s) }, el("bdi", { text: rootsLine(s) }))),
                  statePill(s),
                  el("span", { class: "num", text: s.files ? n(s.files) : "—" }),
                  el("span", { class: "txt-dim", text: savedLine(s) }),
                  el("span", { class: "num-dim", text: s.last_write ? ago(s.last_write) : "never" }),
                ),
              )
            : empty("No store yet."),
        ),
      ),
      el("div", { class: "card-foot", text: `${plural(all.length, "store")} · ${n(files)} files · ${n(chunks)} chunks${disk ? ` · ${bytes(disk)} on disk` : ""}` }),
    );

    const rowsList = (ledger.rows || []).slice(0, 6);
    const activity = el(
      "div",
      { class: "card" },
      el("div", { class: "card-h" }, el("span", { class: "card-t grow", text: "What agents asked" }), lnk("Open ledger →", () => go("ledger"))),
      !rowsList.length
        ? el(
            "div",
            { class: "card-b" },
            el("span", { class: "muted t-sm", text: recordingOn() ? "No agent has asked anything yet. Once one does, every query lands here with what it was sent." : "Recording is paused, so nothing new is listed here. Resume it on the Ledger page." }),
            el("div", { class: "row" }, el("span", { class: "muted t-sm", text: "Try it from Claude Code:" }), el("span", { class: "quote", text: "“use semlith to find where the watcher re-indexes a file”" })),
          )
        : el(
            "div",
            { class: "gl-scroll" },
            el(
              "div",
              { class: "minw480" },
              rowsList.map((r) =>
                el(
                  "div",
                  { class: "gl-row dense gl-activity" },
                  el("span", { class: "t-mono-sm", text: clock(r.at), "data-tip": r.when }),
                  el("span", { class: "t-mono ink2 ell", text: r.client }),
                  el("span", { class: "mono ell t-sm", text: r.query, "data-tip": r.query, "data-tip-full": "" }),
                  el("span", { class: "num-dim", text: plural(r.hits, "hit") }),
                ),
              ),
            ),
          ),
    );

    const attn = attentionItems();
    const attention = el(
      "div",
      { class: "card" },
      el("div", { class: "card-h" }, el("span", { class: "card-t grow", text: "Needs attention" }), meta(attn.length ? `${attn.length} open` : "")),
      !attn.length ? el("div", { class: "all-clear" }, el("span", { class: "check-ok" }, icon(I.check, 11, { w: 3 })), "All clear. Every store is fresh and every agent can reach it.") : null,
      attn.map((a) =>
        el(
          "div",
          { class: "attn-row" },
          dot(a.tone),
          el("span", { class: "col" }, el("span", { class: "t", text: a.title }), el("span", { class: "s", text: a.sub, "data-tip": a.sub })),
          btn({ class: "btn sm t125", onclick: a.onclick }, a.label),
        ),
      ),
    );

    const conns = agents.connections || [];
    const agentsCard = el(
      "div",
      { class: "card" },
      el("div", { class: "card-h" }, el("span", { class: "card-t grow", text: "Agents" }), conns.length || registered.length ? lnk("Manage →", () => go("agents")) : null),
      !conns.length && !registered.length
        ? el(
            "div",
            { class: "card-b" },
            el("span", { class: "muted t-sm", text: found.length ? `Nothing is registered yet. ${found.slice(0, 3).map((c) => c.name).join(", ")}${found.length > 3 ? " and others" : ""} ${found.length === 1 ? "was" : "were"} found on this machine.` : "Nothing is registered yet, and no known client was found on this machine." }),
            btn({ class: "btn sm dark", onclick: () => go("agents", "add") }, "Connect them"),
          )
        : [
            ...conns.map((c) => el("div", { class: "agent-row" }, dot("green"), el("span", { class: "t-m t-sm", text: c.name }), el("span", { class: "t-mono-sm nowrap", text: `${plural(c.queries, "query", "queries")} · ${c.seen ? ago(c.seen) : "no query yet"}` }))),
            ...registered
              .filter((r) => !conns.some((c) => sameClient(c.name, r.name)))
              .slice(0, 4)
              .map((r) => el("div", { class: "agent-row" }, dot("grey"), el("span", { class: "t-m t-sm", text: r.name }), el("span", { class: "t-mono-sm nowrap", text: "registered · not talking now" }))),
          ],
    );

    const events = stores
      .flatMap((s) => (s.events || []).map((e) => ({ ...e, store: s.name })))
      .sort((a, b) => b.at - a.at)
      .slice(0, 6);
    const watcher = el(
      "div",
      { class: "card" },
      el("div", { class: "card-h" }, el("span", { class: "card-t grow", text: "Watcher" }), el("span", { class: "live-word" }, el("span", { class: "dot green slow" }), "live")),
      el(
        "div",
        { class: "feed" },
        events.length ? events.map((e) => el("div", { class: "f" }, el("span", { class: "t", text: clock(e.at), "data-tip": clockFull(e.at) }), el("span", { class: "m", text: `${e.store} · ${e.text}`, "data-tip": `${e.store} · ${e.text}` }))) : el("div", { class: "f" }, el("span", { class: "t", text: "now" }), el("span", { class: "m", text: `${plural(stores.filter((s) => s.watching).length, "store")} watched · nothing has changed since the daemon started` })),
      ),
    );

    return el(
      "div",
      { class: "page" },
      el(
        "div",
        { class: "head" },
        el("div", { class: "titles" }, el("div", { class: "h1", text: "Home" }), el("div", { class: "lead", text: ledger.queries ? "Everything indexed on this machine, and what agents did with it. Nothing leaves it." : "Your index at a glance. It fills in as agents start asking." })),
        el("span", { class: "t-mono-sm", text: `updated ${ago(Math.floor((loadedAt.stores || Date.now()) / 1000))} · ${location.host}` }),
      ),
      checklist,
      kpis,
      el("div", { class: "split s-175-1" }, el("div", { class: "stack" }, storesCard, activity), el("div", { class: "stack" }, attention, agentsCard, watcher)),
    );
  },
};

function sameClient(a, b) {
  const norm = (x) => String(x || "").toLowerCase().replace(/[^a-z]/g, "");
  return norm(a) === norm(b) || norm(a).includes(norm(b)) || norm(b).includes(norm(a));
}

/** What on this machine wants a person, each pointing at where it is handled. */
function attentionItems() {
  const out = [];
  const stores = liveStores();
  for (const s of stores) {
    const r = reviewCount(s.name);
    if (r) out.push({ tone: "amber", title: `${plural(r, "file")} in ${s.name} ${r === 1 ? "waits" : "wait"} for you`, sub: "Held back by the credential scan until you decide", label: "Review", onclick: () => go("store", s.name, "review") });
  }
  for (const s of data.stores?.stores || []) {
    if (s.missing) out.push({ tone: "red", title: `${s.name}'s directory is gone`, sub: s.dir, label: "Details", onclick: () => go("store", s.name, "settings") });
    else if (s.unreadable) out.push({ tone: "red", title: `${s.name} could not be read`, sub: "Its database did not open; other stores are fine", label: "Details", onclick: () => go("store", s.name, "settings") });
    else if ((s.roots || []).some((r) => !r.present)) out.push({ tone: "red", title: `A source of ${s.name} is missing`, sub: (s.roots || []).filter((r) => !r.present).map((r) => tilde(r.path)).join(", "), label: "Re-point", onclick: () => go("store", s.name, "settings") });
  }
  if (data.ledger && data.ledger.intact === false) out.push({ tone: "red", title: "Ledger chain does not verify", sub: "Some rows were edited or removed after they were written", label: "Inspect", onclick: () => go("ledger") });
  const reclaim = stores.filter((s) => reclaimable(s) > 1024 * 1024);
  if (reclaim.length) out.push({ tone: "blue", title: `${bytes(reclaim.reduce((a, s) => a + reclaimable(s), 0))} reclaimable in ${plural(reclaim.length, "store")}`, sub: reclaim.map((s) => s.name).join(", "), label: "Compact", onclick: () => compactStores(reclaim.map((s) => s.name)) });
  if (data.agents && !registeredClients().length) out.push({ tone: "amber", title: "No agent is connected yet", sub: "Register the clients found on this machine in one step", label: "Connect", onclick: () => go("agents", "add") });
  const broken = (data.agents?.doctor || []).filter((c) => c.registered && (c.repair || c.why || (c.disabled_in || []).length));
  if (broken.length) out.push({ tone: "grey", title: `${broken[0].name}${broken.length > 1 ? ` and ${broken.length - 1} more` : ""} cannot reach semlith`, sub: broken[0].why || broken[0].repair || "registered but switched off in some folders", label: "Details", onclick: () => go("agents", "health") });
  const finished = (data.runs?.runs || []).filter((r) => r.status === "done" && r.finished_at && Date.now() / 1000 - r.finished_at < 3600 && r.kind !== "catch-up" && !state.dismissed.has(r.id));
  for (const r of finished.slice(0, 2))
    out.push({ tone: "green", title: `${r.store} finished ${r.kind === "compact" ? "compacting" : "indexing"}`, sub: r.kind === "compact" ? "the space is back" : `${plural(r.indexed || r.total || 0, "file")} searchable now`, label: r.kind === "compact" ? "Open" : "Search it", onclick: () => (state.dismissed.add(r.id), r.kind === "compact" ? go("store", r.store) : searchStore(r.store)) });
  return out;
}

function readPrefs() {
  try {
    return JSON.parse(localStorage.getItem("semlith-prefs") || "{}");
  } catch (_) {
    return {};
  }
}

function savePrefs(patch) {
  try {
    localStorage.setItem("semlith-prefs", JSON.stringify({ ...readPrefs(), ...patch }));
  } catch (_) {
    /* private window */
  }
}

// -------------------------------------------------------------------- stores

const storesUi = { filter: "", kind: "all" };

VIEWS.stores = {
  needs: (route) => (route.parts[0] === "inside" ? ["stores", "coverage", "corpus", "ledger"] : ["stores", "runs", "refused"]),
  live: ["stores", "runs", "refused"],
  render(route) {
    const tab = route.parts[0] === "inside" ? "inside" : "list";
    const all = data.stores?.stores || [];
    return el(
      "div",
      { class: "page" },
      el(
        "div",
        { class: "head" },
        el("div", { class: "titles" }, el("div", { class: "row nowrap gap10" }, el("div", { class: "h1", text: "Stores" }), el("span", { class: "count-chip", text: plural(all.length, "store") })), el("div", { class: "lead", text: "Each store is one index on this machine — its sources, its model, its own writer." })),
        el("div", { class: "row" }, btn({ class: "btn", onclick: adoptFlow }, icon(I.folder, 14), "Adopt existing .semlith"), btn({ class: "btn primary", onclick: () => openWizard({}) }, icon(I.plus, 14, { w: 2.2 }), "New store")),
      ),
      tabs(
        [
          ["list", "All stores"],
          ["inside", "Inside the index"],
        ],
        tab,
        (t) => go("stores", t === "list" ? undefined : t),
      ),
      tab === "list" ? storesList() : insideIndex(),
    );
  },
};

function storesList() {
  const all = data.stores?.stores || [];
  const f = storesUi.filter.toLowerCase();
  const rowsAll = all.filter((s) => (storesUi.kind === "all" || (s.kind || "both") === storesUi.kind) && (!f || s.name.includes(f) || (rootsAll(s) || "").toLowerCase().includes(f)));
  const view = GRID_VIEWS.get("stores") || { page: 1, per: 10, sort: "name", dir: "asc" };
  GRID_VIEWS.set("stores", view);
  const SORT = {
    name: (s) => s.name,
    state: (s) => storeState(s).state,
    files: (s) => s.files || 0,
    chunks: (s) => s.chunks || 0,
    disk: (s) => diskOf(s),
    last: (s) => -(s.last_write || 0),
  };
  const sorted = [...rowsAll].sort((a, b) => {
    const x = SORT[view.sort](a);
    const y = SORT[view.sort](b);
    const sign = view.dir === "desc" ? -1 : 1;
    return (typeof x === "number" ? x - y : String(x).localeCompare(String(y))) * sign;
  });
  const pages = Math.max(1, Math.ceil(sorted.length / view.per));
  view.page = Math.min(view.page, pages);
  const shown = sorted.slice((view.page - 1) * view.per, view.page * view.per);
  const files = all.reduce((a, s) => a + (s.files || 0), 0);
  const chunks = all.reduce((a, s) => a + (s.chunks || 0), 0);
  const reclaimStores = liveStores().filter((s) => reclaimable(s));
  const changed = () => repaint();
  const filterInput = el("input", {
    value: storesUi.filter,
    placeholder: "Filter by name or path",
    "aria-label": "Filter stores",
    "data-keep": "stores-filter",
    oninput: (e) => {
      storesUi.filter = e.target.value;
      view.page = 1;
      repaint();
    },
  });
  return el(
    "div",
    { class: "stack" },
    el(
      "div",
      { class: "row" },
      el("div", { class: "box w240 full-sm" }, icon(I.searchSm, 14, { w: 1.8 }), filterInput),
      seg(
        [
          ["all", "All"],
          ["code", "Code"],
          ["docs", "Docs"],
          ["both", "Both"],
        ],
        storesUi.kind,
        (k) => ((storesUi.kind = k), (view.page = 1), repaint()),
        { label: "Kind" },
      ),
      el("span", { class: "spacer" }),
      el("span", { class: "t-mono-sm", text: `${n(files)} files · ${n(chunks)} chunks` }),
    ),
    el(
      "div",
      { class: "card stbl" },
      el(
        "div",
        { class: "gl-head gl-stores" },
        sortHead("STORE", "name", view, changed),
        sortHead("STATE", "state", view, changed),
        sortHead("FILES", "files", view, changed, "c-files right"),
        sortHead("CHUNKS", "chunks", view, changed, "c-chunks right"),
        sortHead("ON DISK", "disk", view, changed, "c-disk right"),
        el("span", { class: "c-saved", text: "SAVED" }),
        sortHead("WRITTEN", "last", view, changed, "c-written right"),
        el("span"),
      ),
      !shown.length ? el("div", { class: "empty lg" }, "No store matches. ", lnk("Clear the filter", () => ((storesUi.filter = ""), (storesUi.kind = "all"), repaint()))) : null,
      shown.map((s) =>
        el(
          "div",
          {
            class: "gl-row click gl-stores",
            role: "link",
            tabindex: "0",
            onclick: () => go("store", s.name),
            onkeydown: (e) => (e.key === "Enter" ? go("store", s.name) : null),
          },
          el("span", { class: "cellname" }, el("span", { class: "a" }, s.name, " ", el("span", { class: "k", text: kindOf(s) })), el("span", { class: "b ell-start", "data-tip": rootsAll(s) }, el("bdi", { text: rootsLine(s) }))),
          statePill(s),
          el("span", { class: "num c-files", text: s.files ? n(s.files) : "—" }),
          el("span", { class: "num c-chunks", text: s.chunks ? n(s.chunks) : "—" }),
          el(
            "span",
            { class: "st-disk c-disk", "data-tip": s.disk ? "On disk" : null, "data-tip-rows": s.disk ? rows([["database", bytes(s.disk.database)], ["vectors", bytes(s.disk.vectors)], ["exact copy", bytes(s.disk.exact)], ["dead", `${(s.disk.dead_percent || 0).toFixed(1)}%`]]) : null },
            el("span", { class: "mono t-sm", text: s.disk ? bytes(s.disk.total) : "—" }),
            reclaimable(s) ? el("span", { class: "reclaim", text: `${bytes(reclaimable(s))} reclaimable` }) : null,
          ),
          el("span", { class: "txt-dim c-saved", text: savedLine(s) }),
          el("span", { class: "num-dim c-written", text: s.last_write ? ago(s.last_write) : "never" }),
          (() => {
            const m = moreButton(storeMenu(s), `Actions for ${s.name}`);
            m.addEventListener("keydown", (e) => e.stopPropagation());
            return m;
          })(),
        ),
      ),
      el(
        "div",
        { class: "card-foot" },
        el("span", { class: "grow", text: `${sorted.length ? `${(view.page - 1) * view.per + 1}–${Math.min(sorted.length, view.page * view.per)}` : "0"} of ${plural(sorted.length, "store")} · ${liveStores().every((s) => s.watching) ? "every one watched" : `${liveStores().filter((s) => s.watching).length} watched`} · one writer each` }),
        reclaimStores.length ? btn({ class: "btn xs", onclick: () => compactStores(reclaimStores.map((s) => s.name)) }, `Compact ${reclaimStores.length} · reclaim ${bytes(reclaimStores.reduce((a, s) => a + reclaimable(s), 0))}`) : null,
        pager(view, pages, changed),
      ),
    ),
    remoteStores().length
      ? el(
          "div",
          { class: "card" },
          el("div", { class: "card-h" }, el("span", { class: "card-t grow", text: "Remote stores" }), meta("read through Semlith Cloud · never written from here")),
          remoteStores().map((r) =>
            el(
              "div",
              { class: "dl-row" },
              el("span", { class: "col" }, el("span", { class: "t-m t-sm", text: r.name }), el("span", { class: "muted t-xs", text: `${r.host_name} · searched with the local stores; change it in the cloud app` })),
              pill(r.badge, "blue", { dot: false }),
            ),
          ),
          el("div", { class: "card-foot" }, el("span", { class: "grow", text: "Settings › Cloud has their sources, revisions and lag" }), btn({ class: "btn xs", onclick: () => go("settings", "cloud") }, "Open Cloud")),
        )
      : null,
  );
}

// ------------------------------------------------------------ inside the index

const PALETTE = ["#F0A43C", "#4C7088", "#3E9A6E", "#2F4F68", "#B07A2A", "#8A9DAB"];

function insideIndex() {
  const corpus = (data.corpus?.stores || []).filter((c) => !c.error && !c.measuring);
  const measuring = (data.corpus?.stores || []).filter((c) => c.measuring);
  const sum = (k) => corpus.reduce((a, c) => a + (c[k] || 0), 0);
  const files = sum("files");
  const lines = sum("lines");
  const words = sum("words");
  const chars = sum("characters");
  const langs = {};
  for (const c of corpus) for (const l of c.languages || []) langs[l.language] = (langs[l.language] || 0) + l.lines;
  const langList = Object.entries(langs).sort((a, b) => b[1] - a[1]);
  const top = langList.slice(0, 8);
  const pages = Math.round(words / 500);
  const minutes = words / 250;
  const kinds = {};
  for (const c of corpus) for (const k of c.kinds || []) {
    const cur = (kinds[k.name] = kinds[k.name] || { count: 0, units: 0, unit: k.unit });
    cur.count += k.count;
    cur.units += k.units;
  }
  const longest = corpus.map((c) => c.longest_file).filter(Boolean).sort((a, b) => b.lines - a.lines)[0];
  const first = Math.min(...corpus.map((c) => c.first_indexed || Infinity));
  const last = Math.max(...corpus.map((c) => c.last_indexed || 0));
  const symbols = sum("symbols");
  const tiers = {};
  for (const c of corpus) for (const t of c.tiers || []) tiers[t.name] = (tiers[t.name] || 0) + t.count;
  const edgesAll = Object.values(tiers).reduce((a, b) => a + b, 0);
  const settled = (tiers.extracted || 0) + (tiers.resolved || 0);
  const unresolved = {};
  for (const c of corpus) for (const u of c.unresolved_names || []) unresolved[u.name] = (unresolved[u.name] || 0) + u.count;
  const dups = {};
  for (const c of corpus) for (const u of c.ambiguous_worst || []) dups[u.name] = (dups[u.name] || 0) + u.count;
  const months = {};
  for (const c of corpus) for (const m of c.months || []) months[m.month] = (months[m.month] || 0) + m.chunks;
  const runsDone = (data.runs?.history || []).length;
  const median = Math.max(0, ...corpus.map((c) => c.median_query_ms || 0));
  const ledger = data.ledger || {};
  const comment = sum("comment_lines");
  const blank = sum("blank_lines");
  const ago12 = [];
  const now = new Date();
  for (let i = 11; i >= 0; i--) {
    const d = new Date(Date.UTC(now.getUTCFullYear(), now.getUTCMonth() - i, 1));
    ago12.push(d.toISOString().slice(0, 7));
  }
  const maxMonth = Math.max(1, ...ago12.map((m) => months[m] || 0));
  const fmtDay = (unix) => (isFinite(unix) && unix ? new Date(unix * 1000).toLocaleDateString() : "—");
  const errs = (data.corpus?.stores || []).filter((c) => c.error);
  return el(
    "div",
    { class: "stack" },
    el("div", { class: "muted t-sm", text: `Measured from the stores themselves, again after every change, not estimated — ${plural(corpus.length, "store")}, ${n(files)} files.` }),
    measuring.length ? el("div", { class: "notice" }, el("span", { class: "sub", text: `Measuring ${measuring.map((m) => m.store).join(", ")} — the figures appear here when it is done.` })) : null,
    errs.length ? el("div", { class: "notice red" }, el("span", { class: "sub", text: `${errs.map((e) => e.store).join(", ")} could not be measured: ${errs[0].error}` })) : null,
    el(
      "div",
      { class: "q4" },
      kpi("Lines of code", n(lines), "counted, comments and blanks separated out"),
      kpi("Words indexed", n(words), "code, prose, slides, sheets and notebooks"),
      kpi("If it were printed", `${n(pages)} pp`, median ? `a stack of A4 you can search in ${(median / 1000).toFixed(1)} s` : "at 500 words a page"),
      kpi("Reading time", minutes > 1440 ? `${n(minutes / 1440)} days` : `${n(minutes / 60)} hours`, "non-stop at 250 words a minute"),
    ),
    el(
      "div",
      { class: "split s-14-1-1 stretch" },
      el(
        "div",
        { class: "card pad span2" },
        el("div", { class: "row base" }, el("span", { class: "card-t grow", text: "Language mix, by line" }), meta(`${n(lines)} lines · ${plural(langList.length, "language")}`)),
        el(
          "div",
          { class: "segbar" },
          langList.map(([name, v], i) => {
            const seg2 = el("i", { "data-tip": i < 8 ? name : "a smaller language", "data-tip-color": PALETTE[i % PALETTE.length], "data-tip-rows": rows([["lines", n(v)], ["share", pct(v, lines)]]) });
            seg2.style.flex = `${Math.max(0.3, (v / lines) * 100)} 0 0`;
            seg2.style.background = PALETTE[i % PALETTE.length];
            return seg2;
          }),
        ),
        el(
          "div",
          { class: "col gap6" },
          top.map(([name, v], i) => {
            const c = PALETTE[i % PALETTE.length];
            const sw = el("span", { class: "dot sq" });
            sw.style.background = c;
            const b = bar((v / (top[0][1] || 1)) * 100);
            b.firstChild.style.background = c;
            b.setAttribute("data-tip", name);
            b.setAttribute("data-tip-color", c);
            b.setAttribute("data-tip-rows", rows([["lines", n(v)], ["share of all lines", pct(v, lines)], ["rank", `${i + 1} of ${langList.length}`]]));
            return el("div", { class: "lang-row" }, sw, el("span", { text: name }), b, el("span", { class: "right", text: n(v) }), el("span", { class: "pct", text: pct(v, lines) }));
          }),
        ),
        el("div", { class: "grow" }),
        el("div", { class: "muted t-xs", text: `${n(corpus.reduce((a, c) => Math.max(a, c.languages_with_edges || 0), 0))} of these languages carry graph edges as well as search.` }),
      ),
      factCard("What is in the prose", Object.entries(kinds).slice(0, 5).map(([k, v]) => [k, `${plural(v.count, "file")}${v.unit && v.units ? ` · ${n(v.units)} ${v.unit}` : ""}`])),
      factCard("Shape of the code", [
        ["Average line", lines ? `${Math.round(chars / lines)} chars` : "—"],
        ["Comment lines", pct(comment, lines)],
        ["Blank lines", pct(blank, lines)],
        ["Deepest path", `${Math.max(0, ...corpus.map((c) => c.deepest_path || 0))} folders`],
        ["Longest file", longest ? `${n(longest.lines)} lines` : "—"],
      ]),
      factCard("Time in the corpus", [
        ["First read", fmtDay(first)],
        ["Newest write", last ? ago(last) : "—"],
        ["Median retrieval", median ? `${n(median)} ms` : "no retrieval yet"],
        ["Runs remembered", String(runsDone)],
        ["Stores", String(corpus.length)],
      ]),
      factCard("What the graph holds", [
        ["Symbols", n(symbols)],
        ["Settled edges", pct(settled, edgesAll)],
        ["Unresolved calls", n(tiers.unresolved || sum("unresolved"))],
        ["Names defined twice or more", n(sum("ambiguous_names"))],
        ["Edges", n(sum("edges"))],
      ]),
    ),
    el(
      "div",
      { class: "split s-1-14 stretch" },
      el(
        "div",
        { class: "card pad" },
        el("div", { class: "row base" }, el("span", { class: "card-t grow", text: "Chunks added per month" }), el("span", { class: "t-mono-sm", text: "when semlith read it" })),
        el(
          "div",
          { class: "months" },
          ago12.map((m) => {
            const v = months[m] || 0;
            const bar2 = el("i", { class: v ? "" : "zero" });
            bar2.style.height = v ? `${Math.max(3, (v / maxMonth) * 100)}%` : "2px";
            return el("div", { "data-tip": monthName(m), "data-tip-rows": rows([["chunks added", n(v)]]) }, bar2);
          }),
        ),
        el("div", { class: "month-labels" }, ago12.map((m, i) => el("span", { text: i % 3 === 0 || i === 11 ? monthName(m).slice(0, 3) : "" }))),
      ),
      el(
        "div",
        { class: "card pad" },
        el("div", { class: "row base" }, el("span", { class: "card-t grow", text: "Graph health" }), el("span", { class: "t-mono-sm", text: "call edges, every open store" })),
        el(
          "div",
          { class: "segbar" },
          [
            ["extracted", "var(--blue-ink)", "read straight from the syntax tree"],
            ["resolved", "var(--green)", "the name matched exactly one definition"],
            ["ambiguous", "var(--accent)", "the name has several definitions"],
            ["unresolved", "var(--line)", "no definition in any open store"],
          ].map(([k, c, means]) => {
            const s2 = el("i", { "data-tip": k, "data-tip-color": c, "data-tip-rows": rows([["edges", n(tiers[k] || 0)], ["share", pct(tiers[k] || 0, edgesAll)], ["means", means]]) });
            s2.style.flex = `${Math.max(0.2, ((tiers[k] || 0) / Math.max(1, edgesAll)) * 100)} 0 0`;
            s2.style.background = c;
            return s2;
          }),
        ),
        el(
          "div",
          { class: "legend" },
          [
            ["extracted", "var(--blue-ink)"],
            ["resolved", "var(--green)"],
            ["ambiguous", "var(--accent)"],
            ["unresolved", "var(--line)"],
          ].map(([k, c]) => {
            const sw = el("span", { class: "dot sq" });
            sw.style.background = c;
            return el("span", {}, sw, `${k} ${n(tiers[k] || 0)}`);
          }),
        ),
        el(
          "div",
          { class: "auto-fit m170 tb-line" },
          el("div", { class: "col gap6" }, el("span", { class: "eyebrow sm", text: `UNRESOLVED · ${pct(tiers.unresolved || 0, edgesAll)}` }), el("div", { class: "row gap4" }, Object.entries(unresolved).sort((a, b) => b[1] - a[1]).slice(0, 6).map(([k]) => el("span", { class: "tag", text: k }))), el("span", { class: "muted t-xs", text: "Calls into code no open store holds. Hidden from views." })),
          el("div", { class: "col gap4" }, el("span", { class: "eyebrow sm", text: `SEVERAL DEFINITIONS · ${n(sum("ambiguous_names"))}` }), Object.entries(dups).sort((a, b) => b[1] - a[1]).slice(0, 5).map(([k, v]) => el("div", { class: "fact-row" }, el("span", { class: "mono ink2", text: k }), el("span", { class: "mono muted", text: n(v) })))),
        ),
      ),
    ),
    el(
      "div",
      { class: "auto-fit m240" },
      blurb(`${n(chars)} characters`, "Every keystroke in the corpus, kept on this machine and nowhere else."),
      blurb(median ? `${(median / 1000).toFixed(1)} s median` : "no retrieval yet", "The middle of every retrieval this machine has recorded — by meaning, not just words."),
      blurb(ledger.ratio ? `${ledger.ratio.toFixed(1)}× fewer tokens` : "no savings yet", ledger.ratio ? `What agents read versus reading those files whole · coverage ${ledger.coverage}% · ${ledger.tier}` : "Counted once an agent asks something."),
    ),
  );
}

function factCard(title, pairs) {
  return el("div", { class: "card pad" }, el("div", { class: "card-t", text: title }), pairs.map(([k, v]) => el("div", { class: "fact-row" }, el("span", { class: "k", text: k }), el("span", { class: "v", text: v }))));
}

function blurb(v, t) {
  return el("div", { class: "blurb" }, el("span", { class: "v", text: v }), el("span", { class: "t", text: t }));
}

function monthName(ym) {
  const [y, m] = ym.split("-").map(Number);
  return `${["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"][m - 1]} ${y}`;
}

// --------------------------------------------------------------- store detail

const sdUi = { glob: "", type: "all", lang: "all", view: "list", decQ: "", decOut: "all", decBy: "all", histOpen: {}, rename: {} };

VIEWS.store = {
  needs: () => ["stores", "runs", "refused", "decisions", "detail", "ledger"],
  live: ["stores", "runs", "refused", "ledger"],
  // The Runs and Settings tabs draw synchronously, so a live update patches
  // them in place: the run card's buttons and a half-typed name survive it.
  morph: (route) => ["runs", "settings"].includes(route.parts[1]),
  render(route, holder) {
    const name = route.parts[0];
    const tab = route.parts[1] || "overview";
    const s = store(name);
    if (!s) {
      return el("div", { class: "page" }, btn({ class: "back lnk", onclick: () => go("stores") }, icon(I.back, 13, { w: 1.8 }), "All stores"), el("div", { class: "card" }, empty(`There is no store called ${name} on this machine${(data.stores?.stores || []).length ? "" : " yet"}. It may have been renamed or forgotten.`, "lg")));
    }
    const r = activeRun(name);
    const review = reviewCount(name);
    const st = storeState(s);
    const isEmpty = !s.files && !r && !s.missing;
    const page = el(
      "div",
      { class: "page tight" },
      btn({ class: "back lnk", onclick: () => go("stores") }, icon(I.back, 13, { w: 1.8 }), "All stores"),
      el(
        "div",
        { class: "row gap12" },
        el("span", { class: "icon-tile s38" }, icon(I.layers, 18, { w: 1.6 })),
        el("div", { class: "col gap2 grow" }, el("div", { class: "row nowrap gap10" }, el("span", { class: "sd-name", text: s.name }), statePill(s)), el("span", { class: "row gap6 min0 t-mono-sm" }, el("span", { class: "ell-start min0", "data-tip": rootsAll(s) }, el("bdi", { text: rootsLine(s) })), el("span", { class: "nowrap", text: `· ${kindOf(s)}` }))),
        el(
          "div",
          { class: "row" },
          btn({ class: "btn md", onclick: () => searchStore(name) }, icon(I.searchSm, 16, { w: 1.9 }), "Search it"),
          btn({ class: "btn md", disabled: r || isEmpty || s.missing || rootsGone(s) ? true : null, "data-tip": rootsGone(s) ? "Every folder this store reads from is gone — re-point it on Settings" : null, onclick: () => reindexStore(name) }, "Re-index"),
          btn({ class: "btn md dark", onclick: () => openWizard({ store: name }) }, icon(I.plus, 15, { w: 2.2 }), "Add sources"),
        ),
      ),
      tabs(
        [
          ["overview", "Overview"],
          ["files", "Files"],
          ["review", "Review", review || "", "amber"],
          ["runs", "Runs", r ? "1" : "", "blue"],
          ["settings", "Settings"],
        ],
        tab,
        (t) => go("store", name, t === "overview" ? undefined : t),
      ),
    );
    if (isEmpty && tab === "overview") {
      const draft = (s.roots || []).length > 0;
      page.append(
        el(
          "div",
          { class: "drop-hint" },
          el("span", { class: "icon-tile" }, icon(I.upload, 19, { w: 1.7 })),
          el("div", { class: "col gap2", }, el("span", { class: "t-b", text: draft ? "Sources added, not indexed yet" : "This store is empty" }), el("span", { class: "muted t-sm", text: draft ? `${plural(s.roots.length, "source")} waiting. Indexing runs in the background.` : "Add folders, files or a URL. Semlith scans them and asks before anything sensitive goes in." })),
          el("span", { class: "spacer" }),
          btn({ class: "btn md primary", onclick: () => (draft ? reindexStore(name) : openWizard({ store: name })) }, draft ? "Index now" : "Add sources"),
        ),
      );
    }
    const body = { overview: sdOverview, files: sdFiles, review: sdReview, runs: sdRuns, settings: sdSettings }[tab] || sdOverview;
    page.append(body(s, holder));
    return page;
  },
};

function sdOverview(s) {
  const asks = (data.ledger?.rows || []).filter((r) => r.store === s.name).slice(0, 6);
  const readers = Object.entries(detailOf(s).readers_count || {}).sort((a, b) => b[1] - a[1]);
  const langsC = Object.entries(detailOf(s).languages_count || {}).sort((a, b) => b[1] - a[1]).slice(0, 6);
  const v = s.savings;
  return el(
    "div",
    { class: "stack" },
    el(
      "div",
      { class: "q4" },
      kpi("Files", n(s.files || 0), s.files ? `read by ${plural(s.readers || readers.length, "reader")} · ${n(s.lines || 0)} lines` : "nothing yet"),
      kpi("Chunks", n(s.chunks || 0), s.chunks ? "searchable on this machine" : "nothing indexed yet"),
      kpi("On disk", s.disk ? bytes(s.disk.total) : "—", reclaimable(s) ? `${bytes(reclaimable(s))} reclaimable` : "nothing to reclaim"),
      kpi("Saved for agents", v && v.total ? `${short(v.net_tokens)} tokens` : "—", v && v.total ? `coverage ${v.coverage}% · ${v.tier}` : "counted once an agent asks"),
    ),
    el(
      "div",
      { class: "split s-14-1 stretch" },
      el(
        "div",
        { class: "card flexcol" },
        el("div", { class: "card-h" }, el("span", { class: "card-t grow", text: "Sources" }), lnk("+ Add", () => openWizard({ store: s.name }))),
        (s.roots || []).length
          ? s.roots.map((r) => {
              const watching = r.present && s.watching && s.watch !== false;
              return el(
                "div",
                { class: "root-row" },
                el("span", { class: "icon-tile s28" }, icon(I.folder, 14, { w: 1.6 })),
                el("span", { class: "col" }, pathSpan(tilde(r.path), "mono t-sm", r.path), el("span", { class: "muted t-xs", text: !r.present ? "this folder is not there any more" : s.last_write ? `last change ${ago(s.last_write)}` : "not indexed yet" })),
                pill(!r.present ? "missing" : watching ? "watching" : s.files ? "not watching" : "waiting", !r.present ? "red" : watching ? "green" : "grey"),
              );
            })
          : el("div", { class: "root-row" }, el("span", { class: "icon-tile s28" }, icon(I.folder, 14, { w: 1.6 })), el("span", { class: "muted t-sm", text: "no sources yet" }), el("span")),
        el("div", { class: "grow" }),
        el("div", { class: "card-foot", text: shortPath(`${s.dir}/store.db`, 64), "data-tip": `${s.dir}/store.db` }),
      ),
      el(
        "div",
        { class: "card pad" },
        el("div", { class: "row base" }, el("span", { class: "card-t grow", text: "Read as" }), el("span", { class: "t-mono-sm", text: "which reader parsed each file" })),
        readers.length
          ? readers.slice(0, 5).map(([k, c]) => {
              const b = bar((c / Math.max(1, s.files)) * 100);
              b.setAttribute("data-tip", `Read as ${k}`);
              b.setAttribute("data-tip-rows", rows([["share of files", pct(c, s.files)], ["files", n(c)]]));
              return el("div", { class: "reader-row" }, el("span", { text: k }), b, el("span", { class: "right muted", text: pct(c, s.files) }));
            })
          : el("div", { class: "muted t-sm", text: s.files ? `${plural(s.readers || 0, "reader")} · ${plural(s.formats || 0, "format")}` : "Nothing read yet." }),
        el("div", { class: "rule-line" }),
        el("div", { class: "row gap6" }, langsC.map(([k, c]) => el("span", { class: "tag lg" }, k, el("span", { class: "muted", text: pct(c, s.files) })))),
      ),
    ),
    el(
      "div",
      { class: "card" },
      el("div", { class: "card-h" }, el("span", { class: "card-t grow", text: "What agents asked this store" }), v && v.total ? meta(`${short(v.net_tokens)} saved · ${v.coverage}% · ${v.tier}`) : null, lnk("Ledger →", () => go("ledger"))),
      !asks.length ? empty(`Nothing yet. Every query an agent runs against ${s.name} will be listed here with what it was sent.`) : null,
      asks.length
        ? el(
            "div",
            { class: "gl-scroll" },
            el(
              "div",
              { class: "minw480" },
              asks.map((a) => el("div", { class: "gl-row dense gl-asks" }, el("span", { class: "muted", text: clock(a.at), "data-tip": a.when }), el("span", { class: "ink2 ell", text: a.client }), el("span", { class: "ell t-sm", text: a.query, "data-tip": a.query, "data-tip-full": "" }), el("span", { class: "muted right", text: plural(a.hits, "hit") }))),
            ),
          )
        : null,
    ),
  );
}

const FILE_TYPES = [
  ["all", "All"],
  ["tree-sitter", "code"],
  ["text", "text"],
  ["pdf", "pdf"],
  ["office", "office"],
  ["image", "image"],
];

function sdFiles(s, holder) {
  const host = el("div", { class: "stack" });
  const globInput = el("input", {
    class: "mono",
    value: sdUi.glob,
    placeholder: "path or glob, e.g. src/**",
    "aria-label": "Path or glob",
    "data-keep": "sd-glob",
    oninput: (e) => {
      sdUi.glob = e.target.value;
      clearTimeout(sdFiles.t);
      sdFiles.t = setTimeout(reload, 220);
    },
  });
  const langs = Object.keys(detailOf(s).languages_count || {}).sort();
  const langSel = dropdown({ label: "Language", value: sdUi.lang || "all", options: [["all", "All languages"], ...langs.map((l) => [l, l])], onChange: (v) => ((sdUi.lang = v), reload()) });
  const listHost = el("div", {});
  const chipsHost = el("div", { class: "row gap6" });
  const segHost = el("div", {});
  const toolbar = el("div", { class: "row" }, el("div", { class: "box w240 full-sm" }, icon(I.searchSm, 14, { w: 1.8 }), globInput), chipsHost, langSel, el("span", { class: "spacer" }), segHost);
  // Repainted on every reload, so the pressed chip and the view are what the
  // list shows. The tree reads folders, not filters, so it hides them. The
  // glob field stays put, so typing in it never loses focus.
  function paintBar() {
    const tree = sdUi.view === "tree";
    langSel.hidden = tree;
    chipsHost.hidden = tree;
    fill(chipsHost, FILE_TYPES.map(([v, label]) => btn({ class: "chip sm", "aria-pressed": String(sdUi.type === v), onclick: () => ((sdUi.type = v), reload()) }, label)));
    fill(
      segHost,
      seg(
        [
          ["list", "List"],
          ["tree", "Tree"],
        ],
        sdUi.view,
        (v) => ((sdUi.view = v), reload()),
      ),
    );
  }
  host.append(toolbar, listHost);
  const view = GRID_VIEWS.get(`files:${s.name}`) || { page: 1, per: 25, sort: "path", dir: "asc", sel: new Set(), all: false };
  GRID_VIEWS.set(`files:${s.name}`, view);
  let rowsNow = [];
  let total = 0;
  const g = grid({
    key: `files:${s.name}`,
    server: true,
    per: 25,
    select: true,
    id: (r) => r.path,
    caption: `Files in ${s.name}`,
    empty: "No file matches.",
    loadingText: "Reading the file list…",
    onClear: () => {
      sdUi.glob = "";
      sdUi.type = "all";
      sdUi.lang = "all";
      globInput.value = "";
      reload();
    },
    foot: "Forget drops a file's chunks; the file on disk is untouched",
    onQuery: () => reload(true),
    columns: [
      { key: "path", label: "Path", cls: "m", sort: (r) => r.path, render: (r) => pathSpan(relTo(s, r.path), "", r.path) },
      { key: "reader", label: "Read as", sort: (r) => r.reader, render: (r) => el("span", { class: "tag", text: readAs(r) }) },
      { key: "lang", label: "Language", cls: "ms", sort: (r) => r.lang, render: (r) => r.lang || "—" },
      { key: "lines", label: "Lines", cls: "m r", sort: (r) => r.lines, render: (r) => n(r.lines) },
      { key: "chunks", label: "Chunks", cls: "m r", sort: (r) => r.chunks, render: (r) => n(r.chunks) },
      { key: "indexed", label: "Indexed", cls: "ms nowrap", firstDir: "desc", sort: (r) => r.indexed_at, render: (r) => ago(r.indexed_at) },
      { key: "x", label: "", cls: "r", render: (r) => lnk("Forget", () => forgetFiles(s, [r.path]), "amber") },
    ],
    actions: (sel, clear, how) => {
      // "All matching" is every file the filters match, read again in full.
      const paths = async () => (how.all ? ((await api(`/api/files?${filesParams(0, how.total)}`)).files || []).map((f) => f.path) : sel);
      return [
        btn({
          class: "btn xs",
          onclick: async () => {
            const list = await paths();
            copy(list.slice(0, 2000).join("\n"), `Copied ${plural(Math.min(list.length, 2000), "path")}${list.length > 2000 ? " (first 2,000)" : ""}`);
            clear();
          },
        }, "Copy paths"),
        btn({ class: "btn xs", disabled: activeRun(s.name) ? true : null, onclick: async () => (reindexStore(s.name, await paths()), clear()) }, "Re-index"),
        btn({ class: "btn xs danger", onclick: async () => (await forgetFiles(s, await paths())) && clear() }, "Forget"),
      ];
    },
  });

  function filesParams(offset, limit) {
    const params = new URLSearchParams({ store: s.name, limit: String(limit), offset: String(offset), sort: view.sort || "path", dir: view.dir || "asc" });
    const glob = sdUi.glob.trim();
    if (glob) params.append("path", glob.includes("*") ? glob : `**${glob}**`);
    if (sdUi.lang !== "all") params.append("lang", sdUi.lang);
    if (sdUi.type === "tree-sitter") for (const l of codeLangs()) params.append("lang", l);
    else if (sdUi.type !== "all") for (const ext of READER_EXTS[sdUi.type] || []) params.append("ext", ext);
    return params;
  }

  async function reload(fromGrid) {
    if (!fromGrid) {
      // New filters, new result set: a selection of the old one goes.
      view.page = 1;
      view.sel.clear();
      view.all = false;
    }
    paintBar();
    if (sdUi.view === "tree") {
      fill(listHost, filesTree(s));
      return;
    }
    fill(listHost, el("div", { class: "card" }, g.node));
    g.loading();
    const mine = (sdFiles.gen = (sdFiles.gen || 0) + 1);
    try {
      const out = await api(`/api/files?${filesParams((view.page - 1) * view.per, view.per)}`);
      if (mine !== sdFiles.gen) return;
      rowsNow = out.files || [];
      total = out.total || 0;
      g.update(rowsNow, total);
    } catch (e) {
      fill(listHost, el("div", { class: "card" }, errorBox(e.message)));
    }
  }
  holder.onData = (keys) => keys.includes("stores") && sdUi.view === "list" && reload(true);
  reload(true);
  return host;
}

/** Languages the graph reads with tree-sitter, prose formats aside. */
function codeLangs() {
  return (data.about?.graph_languages || []).filter((l) => !["markdown", "json", "yaml", "toml", "html", "css"].includes(l));
}

/** What parsed a file, as the design names it: code goes through tree-sitter. */
function readAs(r) {
  return r.reader === "text" && codeLangs().includes(r.lang) ? "tree-sitter" : r.reader;
}

/** Extensions per reader, for the type chips, from the readers' own lists. */
const READER_EXTS = {
  pdf: ["pdf"],
  office: ["docx", "xlsx", "pptx", "odt", "ods", "odp", "rtf", "epub", "eml", "mbox"],
  image: ["png", "jpg", "jpeg", "gif", "webp"],
  text: ["md", "txt", "rst", "csv", "json", "yaml", "yml", "toml", "html", "xml", "ipynb"],
};

function relTo(s, path) {
  for (const r of (s.roots || []).map((x) => x.path).sort((a, b) => b.length - a.length)) if (path.startsWith(r)) return path.slice(r.length).replace(/^[\\/]/, "");
  return tilde(path);
}

async function forgetFiles(s, paths) {
  const ok = await ask({
    title: paths.length === 1 ? `Forget ${baseName(paths[0])}?` : `Forget ${paths.length} files?`,
    body: "Their chunks, vectors and graph edges leave the store. The files on disk are untouched, and a watched file comes back the next time it changes.",
    ok: "Forget",
    danger: true,
  });
  if (!ok) return false;
  const out = await act(() => post("/api/forget", { store: s.name, paths }), `Forgot ${plural(paths.length, "file")} — chunks dropped, files on disk untouched`);
  if (out) await load("stores", true), repaint();
  return !!out;
}

function filesTree(s) {
  const box = el("div", { class: "card", "data-scroll-keep": "tree" });
  const params = (dir) => {
    const p = new URLSearchParams({ tree: "1", format: "json", sort: "name", store: s.name });
    if (dir) p.set("dir", dir);
    return `/api/files?${p}`;
  };
  function folder(where, dirPath, name, metaText, level, preload) {
    const kids = el("div", { hidden: true });
    let loaded = false;
    const head = btn({ class: "tree-row t-m", "aria-expanded": "false" }, el("span", { class: "c", text: "▸" }), el("span", { class: "grow", text: name }), el("span", { class: "m", text: metaText || "" }));
    head.style.paddingLeft = `${14 + level * 20}px`;
    async function open(want) {
      const on = want === undefined ? kids.hidden : want;
      head.setAttribute("aria-expanded", String(on));
      head.firstChild.textContent = on ? "▾" : "▸";
      kids.hidden = !on;
      if (!on || loaded) return;
      loaded = true;
      fill(kids, el("div", { class: "empty", text: "Reading…" }));
      try {
        const out = await api(params(dirPath));
        const mine = (out.roots || []).find((r) => r.root === where.root) || (out.roots || [])[0];
        children(kids, where, dirPath, mine, level + 1);
      } catch (e) {
        loaded = false;
        fill(kids, errorBox(e.message));
      }
    }
    head.addEventListener("click", () => open());
    if (preload) {
      loaded = true;
      children(kids, where, dirPath, preload, level + 1);
      kids.hidden = false;
      head.setAttribute("aria-expanded", "true");
      head.firstChild.textContent = "▾";
    }
    return el("div", {}, head, kids);
  }
  function children(node, where, dirPath, entry, level) {
    const under = (nm) => (dirPath ? `${dirPath}/${nm}` : nm);
    const items = [];
    for (const d of entry?.dirs || []) items.push(folder(where, under(d.name), d.name, plural(d.files, "file"), level));
    for (const f of entry?.files || []) {
      const row = el("div", { class: "tree-row", title: `${where.root}/${under(f.name)}` }, el("span", { class: "c" }), el("span", { class: "grow", text: f.name }), el("span", { class: "m", text: f.lines ? `${n(f.lines)} lines · ${plural(f.symbols || 0, "symbol")}` : plural(f.chunks || 0, "chunk") }));
      row.style.paddingLeft = `${14 + level * 20}px`;
      items.push(row);
    }
    for (const o of entry?.not_indexed || []) {
      const row = el("div", { class: "tree-row muted" }, el("span", { class: "c" }), el("span", { class: "grow", text: o.name }), el("span", { class: "m", text: `not indexed — ${o.why}` }));
      row.style.paddingLeft = `${14 + level * 20}px`;
      items.push(row);
    }
    if (entry?.more) items.push(el("div", { class: "empty", text: `+ ${n(entry.more)} more in this folder` }));
    if (!items.length) items.push(el("div", { class: "empty", text: "Empty." }));
    fill(node, items);
  }
  (async () => {
    fill(box, el("div", { class: "empty", text: "Reading…" }));
    try {
      const out = await api(params(""));
      const roots = out.roots || [];
      fill(box, roots.length ? roots.map((r) => folder({ root: r.root }, "", tilde(r.root), null, 0, r)) : empty("Nothing indexed yet."));
    } catch (e) {
      fill(box, errorBox(e.message));
    }
  })();
  return box;
}

function decisionsOf(name) {
  const entry = (data.refused?.stores || []).find((x) => x.store === name) || {};
  const made = (data.decisions?.stores || []).find((x) => x.store === name);
  return made ? { ...entry, decisions: made.rows || [] } : entry;
}

// Reader and language counts cost a pass over each store's files, so the
// store page reads them on its own instead of every stores poll.
function detailOf(s) {
  return (data.detail?.stores || []).find((x) => x.name === s.name) || s;
}

const OUTCOME_TONE = { accepted: "blue", "never indexed": "red", "redacted · indexed": "amber", "read as image": "green", "kept out": "grey", skipped: "grey" };

/* One bulk decision pattern for the wizard's Review step and a store's Review
 * tab (spec §3.5, W6): while it is applied the selection bar is its progress
 * line, and it ends on a done or an error line. `b` is
 * { label, total, done, over, error, local, runs }. */
const BULK_BATCH = 100;

function bulkLine(b, dismiss, extra) {
  if (!b.over) {
    return el("div", { class: "selbar bulk", role: "status", "aria-live": "polite" }, el("span", { class: "dot blue pulse" }), el("span", { class: "what", text: `Applying '${b.label}' to ${plural(b.total, "file")}… ${n(b.done)} / ${n(b.total)}` }));
  }
  if (b.error) {
    return el("div", { class: "selbar bulk bad", role: "alert" }, icon(I.x, 13, { w: 2.6 }), el("span", { class: "what", text: `'${b.label}' stopped at ${n(b.done)} / ${n(b.total)}: ${b.error}` }), el("span", { class: "spacer" }), lnk("Dismiss", dismiss));
  }
  return el(
    "div",
    { class: "selbar bulk ok", role: "status", "aria-live": "polite" },
    icon(I.check, 13, { w: 2.6 }),
    el("span", { class: "what", text: b.local ? `'${b.label}' recorded for ${plural(b.total, "file")} — applied when the run starts` : `'${b.label}' applied to ${plural(b.total, "file")}` }),
    extra || null,
    el("span", { class: "spacer" }),
    lnk("Dismiss", dismiss),
  );
}

/** Send `groups` ([decision, files]) in batches, moving `b` along; `step` is
 * called after each batch so the page can redraw the line. */
async function decideInBatches(store, groups, b, step) {
  b.runs = b.runs || [];
  try {
    for (const [decision, files] of groups) {
      for (let i = 0; i < files.length; i += BULK_BATCH) {
        const part = files.slice(i, i + BULK_BATCH);
        const out = await post("/api/refused/decide", { store, files: part, decision });
        if (out && out.run != null) b.runs.push(out.run);
        b.done += part.length - ((out && out.failed) || 0);
        if (out && out.failed) throw new Error(`${plural(out.failed, "file")} could not be decided`);
        step();
      }
    }
  } catch (e) {
    b.error = e.message;
  }
  b.over = true;
  step();
}

function sdReview(s, holder) {
  const entry = decisionsOf(s.name);
  const rowsAll = entry.rows || [];
  const pending = rowsAll.filter((r) => r.reviewable && !r.accepted && !r.kept_out).map((r) => ({ ...r, risk: riskOf(r) })).sort((a, b) => b.risk - a.risk);
  const band = (r) => (r >= 70 ? "high" : r >= 30 ? "medium" : "low");
  const decide = async (files, decision) => {
    const out = await act(() => post("/api/refused/decide", { store: s.name, files, decision }), { in: `Accepted — ${plural(files.length, "file")} indexed on the next pass`, redact: `Redacted — ${plural(files.length, "file")} indexed without the values`, out: `Kept out — ${files.length === 1 ? "it stays" : "they stay"} refused`, reset: `Undone — ${plural(files.length, "file")} back to waiting for you` }[decision]);
    if (out) await loadMany(["refused", "decisions", "stores", "runs"], true), repaint();
  };
  const sel = (sdReview.sel[s.name] = sdReview.sel[s.name] || new Set());
  // The same panel the wizard's review uses: filter by risk, take every
  // suggestion at once, and a list that scrolls inside its card rather than
  // stretching the page by hundreds of rows.
  const risk = sdReview.risk[s.name] || "any";
  const shown = pending.filter((d) => risk === "any" || band(d.risk) === risk);
  const cnt = (b) => pending.filter((d) => b === "any" || band(d.risk) === b).length;
  const SUGGEST = { out: "Keep it out", redact: "Redact & index", in: "Index it" };
  // A selection or every suggestion, sent in batches with its progress in
  // the selection bar; the table is inert until it is over.
  const WORD = { out: "Keep out", redact: "Redact & index", in: "Index" };
  const bulk = sdReview.bulk[s.name];
  const busy = bulk && !bulk.over;
  const runBulk = async (label, groups) => {
    if (sdReview.bulk[s.name] && !sdReview.bulk[s.name].over) return;
    const b = (sdReview.bulk[s.name] = { label, total: groups.reduce((a, [, f]) => a + f.length, 0), done: 0, over: false });
    sel.clear();
    repaint();
    await decideInBatches(s.name, groups, b, repaint);
    await loadMany(["refused", "decisions", "stores", "runs"], true);
    repaint();
  };
  const applySuggestions = () => {
    const by = {};
    for (const d of shown) (by[d.suggest || "out"] = by[d.suggest || "out"] || []).push(d.path);
    return runBulk("Suggestions", Object.entries(by));
  };
  const bulkBar = bulk
    ? bulkLine(bulk, () => (delete sdReview.bulk[s.name], repaint()), bulk.over && bulk.runs && bulk.runs.length ? lnk(bulk.runs.length === 1 ? "See its run" : `See its ${bulk.runs.length} runs`, () => go("store", s.name, "runs")) : null)
    : null;
  const pendingCard = pending.length
    ? el(
        "div",
        { class: "card accent-edge" },
        el("div", { class: "card-h amber" }, el("span", { class: "card-t grow amber-ink", text: "Waiting for your decision" }), el("span", { class: "card-meta amber-ink", text: `${plural(pending.length, "file")} · refused until you decide` })),
        el(
          "div",
          { class: "filterbar", inert: busy || null },
          seg(
            [
              ["any", "Any risk", cnt("any")],
              ["high", "High", cnt("high")],
              ["medium", "Medium", cnt("medium")],
              ["low", "Low", cnt("low")],
            ],
            risk,
            (v) => ((sdReview.risk[s.name] = v), sel.clear(), repaint()),
          ),
          el("span", { class: "spacer" }),
          shown.length ? btn({ class: "btn sm", disabled: busy || null, onclick: applySuggestions, "data-tip": "Each file takes the decision suggested beside it; every one is logged and can be undone" }, `Apply suggestions to ${n(shown.length)}`) : null,
        ),
        el(
          "div",
          { class: "gl-scroll" },
          el(
            "div",
            { class: "minw780" },
            el(
              "div",
              { class: "dec-grid head", inert: busy || null },
              checkbox(shown.length && shown.every((d) => sel.has(d.path)) ? true : shown.some((d) => sel.has(d.path)) ? "mixed" : false, (on) => (shown.forEach((d) => (on ? sel.add(d.path) : sel.delete(d.path))), repaint()), "Select all shown"),
              el("span", { text: `FILE · ${n(shown.length)} shown · highest risk first` }),
              el("span", { text: "RISK IF INDEXED" }),
              el("span", { text: "DECISION" }),
            ),
            sel.size && !busy
              ? el(
                  "div",
                  { class: "selbar" },
                  el("span", { class: "what", text: `${plural(sel.size, "file")} selected` }),
                  lnk("Clear", () => (sel.clear(), repaint())),
                  el("span", { class: "spacer" }),
                  ["out", "redact", "in"].map((v) => btn({ class: `btn sm${v === "redact" ? " amber" : v === "in" ? " dark" : ""}`, onclick: () => runBulk(WORD[v], [[v, [...sel.values()]]]) }, WORD[v])),
                )
              : bulkBar,
            el(
              "div",
              { class: "dec-list", "data-scroll-keep": `sd-dec-${s.name}`, inert: busy || null },
              !shown.length ? empty("No file at this risk.", "lg") : null,
              shown.map((d) => {
                const b = band(d.risk);
                const on = sel.has(d.path);
                return el(
                  "div",
                  { class: "dec-grid" },
                  checkbox(on, () => (on ? sel.delete(d.path) : sel.add(d.path), repaint())),
                  el(
                    "div",
                    { class: "col gap4 min0" },
                    el("div", { class: "row min0" }, pathSpan(relTo(s, d.path), "p", d.path), d.likely ? pill(d.likely, d.tone || "amber", { dot: false }) : null),
                    el("span", { class: "why" }, el("b", { text: d.kind || classWord(d.class) }), ` · ${d.why || d.rule}`),
                    d.evidence || (d.matches || [])[0] ? el("span", { class: "evidence", text: d.evidence || maskedOf(d.matches[0]) }) : null,
                  ),
                  el("div", { class: "col gap4 risk" }, el("div", { class: "row base nowrap gap6" }, el("span", { class: `risk-pct ${b}`, text: `${d.risk}%` }), el("span", { class: "t-mono-sm", text: b })), bar(Math.max(3, d.risk), `h4 ${b === "high" ? "red" : b === "medium" ? "amber" : "green"}`)),
                  el(
                    "div",
                    { class: "col gap4 acts" },
                    el(
                      "div",
                      { class: "row gap6 nowrap" },
                      btn({ class: "btn sm", onclick: () => decide([d.path], "out") }, "Keep out"),
                      btn({ class: "btn sm amber", disabled: d.class === "credential" ? true : null, onclick: () => decide([d.path], "redact") }, "Redact & index"),
                      btn({ class: "btn sm dark", disabled: d.class === "credential" ? true : null, onclick: () => decide([d.path], "in") }, "Index"),
                    ),
                    el("span", { class: "muted t-xs", text: `Suggested · ${SUGGEST[d.suggest || "out"]}` }),
                  ),
                );
              }),
            ),
          ),
        ),
      )
    : el("div", { class: "stack" }, bulkBar, el("div", { class: "notice green" }, icon(I.check, 14, { w: 2.6 }), "Nothing waits for you. New files that look sensitive will show up here before they are indexed."));

  const decisions = (entry.decisions || rowsAll.filter((r) => !(r.reviewable && !r.accepted && !r.kept_out)).map(decisionFromRow)).map((d, i) => ({ ...d, i, id: `${i}:${d.path}` }));
  const q = sdUi.decQ.toLowerCase();
  const filtered = decisions.filter((d) => (sdUi.decOut === "all" || d.outcome === sdUi.decOut) && (sdUi.decBy === "all" || (sdUi.decBy === "you" ? d.can_undo : !d.can_undo)) && (!q || `${d.path} ${d.why}`.toLowerCase().includes(q)));
  const g = grid({
    key: `dec:${s.name}`,
    caption: `Decisions for ${s.name}`,
    rows: filtered,
    select: true,
    canSelect: (d) => d.can_undo,
    id: (d) => d.path,
    empty: "No decision matches.",
    onClear: () => ((sdUi.decQ = ""), (sdUi.decOut = "all"), (sdUi.decBy = "all"), repaint()),
    columns: [
      { key: "path", label: "Path", cls: "m cap-path", sort: (d) => d.path.toLowerCase(), render: (d) => pathSpan(relTo(s, d.path), "", d.path) },
      { key: "outcome", label: "Outcome", sort: (d) => d.outcome, render: (d) => pill(d.outcome, OUTCOME_TONE[d.outcome] || "grey", { dot: false }) },
      { key: "why", label: "Why", cls: "dim c-why", sort: (d) => (d.why || "").toLowerCase(), render: (d) => d.why || "" },
      { key: "by", label: "By", cls: "ms nowrap", sort: (d) => d.by, render: (d) => (d.at && d.by === "you" ? `you · ${ago(d.at)}` : d.by) },
      { key: "x", label: "", cls: "r", render: (d) => (d.can_undo ? lnk("Undo", () => decide([d.path], "reset")) : null) },
    ],
    actions: (selIds, clear) => [btn({ class: "btn xs", onclick: () => (decide(selIds, "reset"), clear()) }, "Undo selected")],
  });
  const qInput = el("input", {
    value: sdUi.decQ,
    placeholder: "Filter by path or reason",
    "data-keep": "dec-q",
    "aria-label": "Filter decisions",
    oninput: (e) => {
      sdUi.decQ = e.target.value;
      repaint();
    },
  });
  return el(
    "div",
    { class: "stack" },
    pendingCard,
    el(
      "div",
      { class: "card" },
      el("div", { class: "card-h" }, el("span", { class: "card-t grow", text: "Decisions" }), meta("yours and the rules' · undo any of yours")),
      el(
        "div",
        { class: "filterbar" },
        el("div", { class: "box h28 w220 full-sm" }, icon(I.searchSm, 13, { w: 1.8 }), qInput),
        dropdown({
          label: "Outcome",
          value: sdUi.decOut,
          options: ["all", "never indexed", "skipped", "read as image", "accepted", "redacted · indexed", "kept out"].map((o) => [o, o === "all" ? "All outcomes" : o]),
          onChange: (v) => ((sdUi.decOut = v), repaint()),
        }),
        seg(
          [
            ["all", "All"],
            ["you", "Yours"],
            ["rule", "Rules"],
          ],
          sdUi.decBy,
          (v) => ((sdUi.decBy = v), repaint()),
        ),
      ),
      g.node,
    ),
  );
}
sdReview.sel = {};
sdReview.risk = {};
sdReview.bulk = {};

/** A refusal row as a Decisions-table row, for a daemon that sends no table. */
function decisionFromRow(r) {
  const outcome = r.accepted ? (r.accepted === "redacted" ? "redacted · indexed" : r.accepted === "refused" || r.accepted === "kept" ? "kept out" : "accepted") : r.class === "credential" ? "never indexed" : r.class === "dummy" ? "accepted" : r.class === "unindexable" && /image/.test(r.rule) ? "read as image" : "skipped";
  const mine = !!r.accepted;
  return { path: r.path, outcome, why: r.rule, by: mine ? "you" : "rules", at: r.last_seen, can_undo: mine };
}

function sdRuns(s, holder) {
  const live = runsOf(s.name).filter((r) => LIVE_RUN.has(r.status));
  // A finished run this daemon still holds says more than its history line
  // (what it indexed, how long it took), so it is read over the line when the
  // two are the same run. Run ids restart with the daemon, hence the time.
  const doneHere = runsOf(s.name).filter((r) => !LIVE_RUN.has(r.status));
  const same = (h, r) => h.id === r.id && Math.abs((h.finished || 0) - (r.finished_at || 0)) < 5;
  const hist = [
    ...(data.runs?.history || []).filter((h) => h.store === s.name).map((h) => {
      const r = doneHere.find((x) => same(h, x));
      return r ? { ...h, ...r, log: (h.log || []).length ? h.log : r.log } : h;
    }),
    ...doneHere.filter((r) => !(data.runs?.history || []).some((h) => h.store === s.name && same(h, r))),
  ].sort((a, b) => (b.finished || b.finished_at || 0) - (a.finished || a.finished_at || 0));
  const host = el("div", { class: "stack" });
  for (const r of live) {
    const paused = r.status === "paused" || r.status === "pausing";
    const p = runPct(r);
    const b = bar(p, "h8 accent grow");
    b.setAttribute("data-tip", `Indexing · ${Math.floor(p)}%`);
    b.setAttribute("data-tip-rows", runRows(r));
    const log = el("div", { class: "log rounded", "data-scroll-keep": `log-${r.id}`, "data-morph-keep": `log-${r.id}` });
    followLog(r, log);
    host.append(
      el(
        "div",
        { class: "card blue-edge pad" },
        el(
          "div",
          { class: "row" },
          el("span", { class: "card-t", text: r.status === "queued" ? "Waiting to start" : r.status === "review" ? "Held for review" : "Running now" }),
          pill(r.status === "pausing" ? "pausing" : paused ? "paused" : r.status === "review" ? "waiting for review" : r.status === "queued" ? `queued${r.position ? ` · ${r.position} in line` : ""}` : r.kind === "compact" ? "compacting" : r.kind === "catch-up" ? "catching up" : "indexing", paused || r.status === "review" || r.status === "queued" ? "amber" : "green", { pulse: r.status === "running" }),
          el("span", { class: "spacer" }),
          // Every control is drawn and hidden when it does not apply, so the
          // card keeps its shape across states and a live patch never moves
          // the button someone just pressed.
          btn({ class: "btn sm primary", hidden: r.status === "review" ? null : true, onclick: () => go("store", s.name, "review") }, "Review and start"),
          btn({ class: "btn sm", hidden: r.status === "review" ? null : true, onclick: () => runControl(r, "start") }, "Start indexing"),
          btn({ class: "btn sm", hidden: ["running", "paused", "pausing"].includes(r.status) ? null : true, "data-keep": `run-ctl-${r.id}`, onclick: () => runControl(r, paused ? "resume" : "pause") }, paused ? "Resume" : "Pause"),
          btn({ class: "btn sm danger-soft", "data-keep": `run-stop-${r.id}`, onclick: () => stopRun(r) }, r.status === "queued" ? "Take out of the queue" : "Stop…"),
        ),
        el("div", { class: "row nowrap gap12" }, b, el("span", { class: "mono t-b", text: `${Math.floor(p)}%` })),
        el("div", { class: "t-mono-sm", "data-tip": runRangeTip(r), text: [r.total ? `${n(r.scanned)} / ${n(r.total)} files` : "", `${chunksOf(r)} chunks`, r.images_total ? `${n(r.images || 0)} / ${n(r.images_total)} images` : "", r.rate != null ? `${perSecond(r.rate)} chunks/s` : "", laneRatesText(r), r.backlog ? `${n(r.backlog)} waiting` : "", r.status === "running" ? (truthRun(r) && r.eta_ms != null && r.eta_ms >= 10000 ? `${runLeftText(r)} left` : runLeftText(r)) : ""].filter(Boolean).join(" · ") }),
        el("div", { class: "muted t-sm", text: phaseText(r), hidden: r.phase ? null : true }),
        log,
      ),
    );
  }
  host.append(
    el(
      "div",
      { class: "card" },
      el("div", { class: "card-h" }, el("span", { class: "card-t grow", text: "History" }), meta("runs live in the daemon, not the page · kept across restarts")),
      !hist.length ? empty("No finished run yet.") : null,
      hist.length
        ? el(
            "div",
            { class: "gl-scroll" },
            el(
              "div",
              { class: "minw520" },
              hist.slice(0, 30).map((h, i) => {
                const key = `${s.name}:${h.id}:${h.started || h.started_at}`;
                const open = sdUi.histOpen[key] ?? i === 0;
                const result = h.result || h.status;
                const finished = h.finished || h.finished_at;
                const kind = h.kind_label || runKindLabel(h);
                return el(
                  "div",
                  { class: "line-row" },
                  btn(
                    { class: "hist-row", "aria-expanded": String(open), onclick: () => ((sdUi.histOpen[key] = !open), repaint()) },
                    el("span", { class: `caret${open ? " open" : ""}` }, icon(I.chevRight, 13, { w: 2 })),
                    el("span", { class: "t-mono-sm", text: finished ? dayClock(finished) : "—" }),
                    el("span", { class: "col" }, el("span", { class: "t-m t-sm", text: kind }), el("span", { class: "t-mono-sm ell", text: runLine(h) })),
                    pill(result, result === "done" ? "green" : result === "stopped" ? "amber" : result === "failed" ? "red" : "grey", { dot: false }),
                  ),
                  open
                    ? el(
                        "div",
                        { class: "hist-open" },
                        h.stages ? el("span", { class: "t-mono-sm", text: stagesText(h.stages) }) : null,
                        (h.log || []).length ? el("div", { class: "log rounded" }, h.log.slice(-40).map((ev) => {
                          const [a, b, c, tone] = typeof ev === "string" ? ["", "", ev, ""] : ev.event ? logParts(ev) : [ev.at ? clock(ev.at) : "", ev.level || "", ev.text || "", ev.level === "error" ? "bad" : ev.level === "warn" ? "warn" : "info"];
                          return el("div", { class: "ln" }, el("span", { class: "a", text: a }), el("span", { class: `b ${tone}`, text: b }), el("span", { class: "c", text: c }));
                        })) : el("span", { class: "muted t-xs", text: "No log was kept for this run." }),
                      )
                    : null,
                );
              }),
            ),
          )
        : null,
    ),
  );
  return host;
}

function runKindLabel(h) {
  if (h.kind === "compact") return "Compact";
  if (h.kind === "catch-up") return "Watcher catch-up";
  if (h.files && h.files.length) return `Re-index ${plural(h.files.length, "file")}`;
  return h.files_before || h.chunks_before ? "Index" : "First index";
}

function runLine(h) {
  if (h.kind === "compact" && h.summary?.compact) {
    const c = h.summary.compact;
    const total = (f) => (f ? f.database + f.exact + f.vectors : 0);
    return `${bytes(total(c.before))} → ${bytes(total(c.after))}`;
  }
  const ms = h.elapsed_ms ? h.elapsed_ms - (h.queued_ms || 0) : h.started && h.finished ? (h.finished - h.started) * 1000 : 0;
  // History keeps whole seconds; a run inside one second says so rather than
  // claiming a time it did not measure.
  const took = ms ? ` · took ${spellTook(ms)}` : h.finished || h.finished_at ? " · took under a second" : "";
  const indexed = h.indexed ?? h.files_indexed ?? (typeof h.files === "number" ? h.files : 0);
  return `${n(indexed)} indexed · ${n(h.chunks || 0)} chunks${took}`;
}

function stagesText(st) {
  const secs = (ms) => `${((ms || 0) / 1000).toFixed(1)} s`;
  const lanes = Object.keys(st.embed_wait_ms || {})
    .sort()
    .map((lane) => `embed (${laneName(lane)}) ${secs(st.embed_wait_ms[lane])}`);
  return `stages over ${secs(st.wall_ms)}: walk ${secs(st.walk_ms)} · read+hash ${secs(st.read_ms)} · scan ${secs(st.extract_ms)} · parse+chunk ${secs(st.parse_ms)} · tokenize ${secs(st.tokenize_ms)}${lanes.length ? ` · ${lanes.join(" · ")}` : ""} · write ${secs(st.write_ms)}`;
}

function sdSettings(s) {
  const draft = sdUi.rename[s.name] ?? s.name;
  const valid = NAME_RE.test(draft) && draft !== s.name && !(data.stores?.stores || []).some((x) => x.name === draft);
  const renameInput = el("input", {
    class: "inp mono grow",
    value: draft,
    spellcheck: "false",
    "aria-label": "Store name",
    "data-keep": "sd-rename",
    oninput: (e) => {
      const v = e.target.value.toLowerCase().replace(/\s+/g, "-");
      if (v !== e.target.value) e.target.value = v;
      sdUi.rename[s.name] = v;
      saveBtn.disabled = !(NAME_RE.test(v) && v !== s.name && !(data.stores?.stores || []).some((x) => x.name === v));
    },
    onkeydown: (e) => e.key === "Enter" && !saveBtn.disabled && rename(),
  });
  const rename = async () => {
    const to = sdUi.rename[s.name];
    const out = await act(() => post("/api/store/settings", { store: s.name, rename: to }), `Renamed to ${to}`);
    if (out) {
      delete sdUi.rename[s.name];
      await loadMany(["stores", "runs", "refused"], true);
      go("store", to, "settings");
    }
  };
  const saveBtn = btn({ class: "btn dark", disabled: valid ? null : true, onclick: rename }, "Rename");
  const setting = async (patch, word) => {
    const out = await act(() => post("/api/store/settings", { store: s.name, ...patch }), word);
    if (out) await load("stores", true), repaint();
  };
  const lean = s.lean || { code: "code", docs: "docs" }[s.kind] || "either";
  const reclaim = reclaimable(s);
  const missing = (s.roots || []).filter((r) => !r.present);
  const outside = s.trusted === false;
  return el(
    "div",
    { class: "auto-fit m340 align-start" },
    el(
      "div",
      { class: "card pad16" },
      el("span", { class: "card-t", text: "General" }),
      el("div", { class: "field-l" }, el("span", { class: "eyebrow", text: "Name" }), el("div", { class: "row nowrap" }, renameInput, saveBtn), el("span", { class: "muted t-xs", text: "Agents pick stores by name; a rename shows up on their next call." })),
      el(
        "div",
        { class: "field-l" },
        el("span", { class: "eyebrow", text: "Search leans to" }),
        seg(
          [
            ["code", "Code"],
            ["docs", "Docs"],
            ["either", "Neither"],
          ],
          lean,
          (v) => setting({ lean: v }, `Search in ${s.name} now leans to ${v === "either" ? "neither" : v}`),
        ),
      ),
      el(
        "div",
        { class: "col gap4 tb-line" },
        toggleRow(s.watch !== false, "Watch for changes", s.watching === false && s.watch !== false && s.stopped_because ? `Stopped: ${s.stopped_because}` : "Re-index a file the moment it is saved.", (v) => setting({ watch: v }, `Watch for changes — ${v ? "on" : "off"}`), { plain: true }),
        toggleRow(s.record !== false, "Record retrievals", recordingOn() ? "Keep this store's queries in the local ledger." : "Recording is paused for the whole machine on the Ledger page.", (v) => setting({ record: v }, `Record retrievals — ${v ? "on" : "off"}`), { plain: true }),
        toggleRow(s.gitignore !== false, "Respect .gitignore", "Skip what each repository already ignores, on every run and on the watcher.", (v) => setting({ gitignore: v }, `Respect .gitignore — ${v ? "on" : "off"}`), { plain: true }),
      ),
    ),
    el(
      "div",
      { class: "stack" },
      el(
        "div",
        { class: "card pad" },
        el("span", { class: "card-t", text: "Maintenance" }),
        el("div", { class: "maint-row" }, el("span", { class: "col grow" }, el("span", { class: "t", text: "Compact" }), el("span", { class: "s", text: reclaim ? `${bytes(reclaim)} of deleted chunks can be reclaimed. Searches keep working while it runs.` : `Nothing to reclaim. The daemon compacts on its own past ${data.runs?.compaction?.threshold_percent ?? 25}%.` })), btn({ class: "btn", disabled: reclaim ? null : true, onclick: () => compactStores([s.name]) }, "Compact now")),
        el("div", { class: "maint-row" }, el("span", { class: "col grow" }, el("span", { class: "t", text: "Re-index everything" }), el("span", { class: "s", text: "Reads every source again. Unchanged files are skipped by hash." })), btn({ class: "btn", disabled: activeRun(s.name) || !(s.roots || []).length || rootsGone(s) ? true : null, onclick: () => reindexStore(s.name) }, "Re-index")),
      ),
      missing.length || outside
        ? el(
            "div",
            { class: "card pad" },
            el("span", { class: "card-t", text: "Where it reads from" }),
            missing.map((r) =>
              el(
                "div",
                { class: "maint-row" },
                el("span", { class: "col grow" }, el("span", { class: "t", text: `${tilde(r.path)} is not there` }), el("span", { class: "s", text: "If the folder moved, point the store at its new place. Nothing is re-embedded." })),
                btn({ class: "btn", onclick: () => repoint(s, r.path) }, "Re-point…"),
              ),
            ),
            outside
              ? el(
                  "div",
                  { class: "maint-row" },
                  el("span", { class: "col grow" }, el("span", { class: "t", text: "Outside the store home, not trusted" }), el("span", { class: "s", text: "This daemon opened it because it was asked to. Trust it so `semlith mcp` opens it too." })),
                  btn({ class: "btn", onclick: () => act(() => post("/api/trust", { path: s.dir }), "Trusted").then(() => load("stores", true)).then(repaint) }, "Trust"),
                )
              : null,
          )
        : null,
      el(
        "div",
        { class: "card pad red-edge" },
        el("div", { class: "maint-row" }, el("span", { class: "col grow" }, el("span", { class: "t-b red", text: "Forget this store" }), el("span", { class: "s", text: "Deletes the index and its ledger rows. The files it read stay exactly where they are." })), btn({ class: "btn danger", onclick: async () => (await forgetStore(s.name)) && go("stores") }, "Forget…")),
      ),
    ),
  );
}

async function repoint(s, from) {
  const to = await pickFolder({ title: `Where did ${baseName(from)} go?`, ok: "Use this folder", hint: `${s.name} will read from the folder you pick. Its vectors stay as they are.` });
  if (!to) return;
  const out = await act(() => post("/api/root", { store: s.name, root: to, from }), (o) => o.message || "Re-pointed");
  if (out) await load("stores", true), repaint();
}

// -------------------------------------------------------------------- search

/* Four modes over one index. Ranked is the fused search an agent's
 * `semlith_search` runs, Brief is exactly what `semlith_brief` hands back, Exact
 * is every matching line (`exact: true`), and Pattern is a tree-sitter query
 * over one language's files (`semlith_pattern`). */
const sr = {
  query: "",
  mode: "ranked",
  store: "",
  prefer: "either",
  k: 8,
  budget: 1500,
  lang: "",
  path: "",
  patternLang: "",
  result: null,
  error: "",
  busy: false,
  sel: null,
  whole: false,
  detail: null,
  scopeOpen: false,
};

try {
  sr.recentSeed = JSON.parse(sessionStorage.getItem("semlith-recents") || "[]");
  state.recents = sr.recentSeed;
} catch (_) {
  /* private window */
}

const BUDGETS = [500, 1000, 1500, 3000, 6000];

VIEWS.search = {
  fill: true,
  needs: () => ["stores", "languages"],
  live: [],
  render(route, holder) {
    if (state.pending.searchStore !== undefined) {
      sr.store = state.pending.searchStore;
      delete state.pending.searchStore;
    }
    if (state.pending.searchQuery) {
      sr.query = state.pending.searchQuery;
      delete state.pending.searchQuery;
      setTimeout(() => runSearch(), 0);
    }
    if (sr.store && !store(sr.store)) sr.store = "";
    const root = el("div", { class: "col fillpage grow min0" });
    root.style.height = "100%";
    const bar = el("div", { class: "sr-bar" });
    const body = el("div", { class: "col grow min0" });
    root.append(bar, body);
    const input = el("input", {
      value: sr.query,
      placeholder: sr.mode === "pattern" ? "(call_expression function: (identifier) @f)" : sr.mode === "exact" ? "An identifier, an error string or a regular expression" : "Ask in words, or paste an identifier",
      "aria-label": "Search query",
      "data-keep": "sr-q",
      class: sr.mode === "pattern" ? "mono" : null,
      oninput: (e) => {
        sr.query = e.target.value;
        clearBtn.hidden = !sr.query;
      },
      onkeydown: (e) => {
        if (e.key === "Enter") runSearch();
        if (e.key === "Escape") {
          sr.query = "";
          sr.result = null;
          input.value = "";
          paintBody();
        }
      },
    });
    const clearBtn = btn({ class: "x-btn", hidden: !sr.query, "aria-label": "Clear", onclick: () => ((sr.query = ""), (sr.result = null), (input.value = ""), (clearBtn.hidden = true), paintBody(), input.focus()) }, icon(I.x, 12, { w: 2.2 }));
    const scopeBtn = btn(
      { class: "scope-btn", "aria-haspopup": "menu", "aria-expanded": "false", onclick: () => menu.open(scopeBtn, scopeItems(), { width: 230 }) },
      el("span", { class: "muted hide-sm", text: "in" }),
      el("span", { class: "v", text: sr.store || "all stores" }),
      icon(I.chevDown, 12, { w: 2 }),
    );
    const scopeItems = () => [
      { label: "All stores", hint: plural(liveStores().length, "store"), checked: !sr.store, onclick: () => ((sr.store = ""), repaint(), sr.query && runSearch()) },
      ...liveStores().map((s) => ({ label: s.name, hint: s.files ? `${n(s.files)} files` : "empty", checked: sr.store === s.name, onclick: () => ((sr.store = s.name), (sr.prefer = s.lean || sr.prefer), repaint(), sr.query && runSearch()) })),
      ...remoteStores().map((r) => ({ label: r.name, hint: r.badge, checked: sr.store === r.name, onclick: () => ((sr.store = r.name), repaint(), sr.query && runSearch()) })),
    ];
    setTimeout(() => input.focus(), 0);
    const langs = (data.languages?.languages || []).map((l) => l.name);
    const langChip = sr.lang
      ? el("span", { class: "filter-chip" }, `language ${sr.lang}`, btn({ "aria-label": "Remove language", onclick: () => ((sr.lang = ""), repaint(), sr.query && runSearch()) }, icon(I.x, 10, { w: 2.4 })))
      : (() => {
          const b = btn({ class: "chip dashed", "aria-haspopup": "menu", onclick: () => menu.open(b, langs.map((l) => ({ label: l, onclick: () => ((sr.lang = l), repaint(), sr.query && runSearch()) })), { width: 200, alignLeft: true }) }, "+ language");
          return b;
        })();
    const pathInput = el("input", {
      value: sr.path,
      placeholder: "src/** or *.md",
      "aria-label": "Path filter",
      "data-keep": "sr-path",
      onkeydown: (e) => {
        if (e.key === "Enter") {
          sr.path = e.target.value.trim();
          sr.query && runSearch();
          repaint();
        }
        if (e.key === "Escape") ((sr.path = ""), (sr.pathOpen = false), repaint());
      },
    });
    const pathChip = sr.path || sr.pathOpen
      ? el("span", { class: "filter-chip" }, "path", pathInput, btn({ "aria-label": "Remove path filter", onclick: () => ((sr.path = ""), (sr.pathOpen = false), repaint(), sr.query && runSearch()) }, icon(I.x, 10, { w: 2.4 })))
      : btn({ class: "chip dashed", onclick: () => ((sr.pathOpen = true), repaint(), setTimeout(() => shell.main.querySelector('[data-keep="sr-path"]')?.focus(), 0)) }, "+ path");
    const patternLang = dropdown({ label: "Pattern language", value: sr.patternLang || "", options: [["", "language…"], ...(data.about?.graph_languages || langs).map((l) => [l, l])], onChange: (v) => (sr.patternLang = v) });
    // Beside the field where there is room; on a narrow page at the start of
    // the row below, as a row that wraps put it over that row.
    const narrow = ((shell.main && shell.main.clientWidth) || 1200) < 760;
    const modeSeg = seg(
          [
            ["ranked", "Ranked", null, "Spans ranked by meaning, words and the graph"],
            ["brief", "Brief", null, "What an agent's semlith_brief call returns"],
            ["exact", "Exact", null, "Every matching line, like grep -E"],
            ["pattern", "Pattern", null, "A tree-sitter query over one language, like semlith_pattern"],
          ],
          sr.mode,
          (m) => {
            sr.mode = m;
            sr.result = null;
            sr.sel = null;
            repaint();
            if (sr.query && m !== "pattern") runSearch();
          },
          { cls: "lg", label: "Mode" },
        );
    fill(
      bar,
      el(
        "div",
        { class: "row top" },
        el("div", { class: "sr-field" }, icon(I.searchSm, 16, { w: 1.8 }), input, clearBtn, el("span", { class: "vrule hide-sm" }), scopeBtn, btn({ class: "btn dark tight-sm", onclick: runSearch }, "Search")),
        narrow ? null : modeSeg,
      ),
      el(
        "div",
        { class: "row" },
        narrow ? modeSeg : null,
        sr.mode === "pattern"
          ? [patternLang, el("span", { class: "muted t-xs", text: "Captures every node the query matches in that language's indexed files." })]
          : [
              el(
                "div",
                { class: "dial" },
                el("span", { class: "eyebrow", text: "LEAN TO" }),
                [
                  ["code", "code"],
                  ["docs", "docs"],
                  ["either", "either"],
                ].map(([v, label]) => btn({ "aria-pressed": String(sr.prefer === v), onclick: () => ((sr.prefer = v), repaint(), sr.query && runSearch()) }, label)),
              ),
              sr.mode === "ranked"
                ? el(
                    "div",
                    { class: "dial" },
                    el("span", { class: "eyebrow", text: "RESULTS" }),
                    String(sr.k),
                    btn({ class: "pm", "aria-label": "Fewer results", onclick: () => ((sr.k = Math.max(1, sr.k - 1)), repaint()) }, "−"),
                    btn({ class: "pm", "aria-label": "More results", onclick: () => ((sr.k = Math.min(50, sr.k + 1)), repaint()) }, "+"),
                  )
                : null,
              sr.mode !== "exact"
                ? btn({ class: "dial", "data-tip": "What one answer may cost an agent", onclick: () => ((sr.budget = BUDGETS[(BUDGETS.indexOf(sr.budget) + 1) % BUDGETS.length]), repaint(), sr.query && runSearch()) }, el("span", { class: "eyebrow", text: "BUDGET" }), `${n(sr.budget)} tok`)
                : null,
            ],
        langChip,
        pathChip,
        el("span", { class: "spacer" }),
        sr.result && !sr.result.error ? el("span", { class: "row gap6 nowrap t-mono-sm" }, el("span", { class: "dot blue" }), resultMeta()) : null,
      ),
    );
    function paintBody() {
      fill(body, searchBody());
    }
    holder.paintBody = paintBody;
    searchView.paintBody = paintBody;
    paintBody();
    return root;
  },
};

const searchView = {};

function resultMeta() {
  const r = sr.result;
  if (!r) return "";
  const where = sr.store || "all stores";
  if (sr.mode === "brief") return `one call · ${n((r.micros || 0) / 1000)} ms · ${where}`;
  if (sr.mode === "exact" || sr.mode === "pattern") return `${plural((r.matches || []).length, "line")} in ${plural(r.files || 0, "file")}${r.truncated ? " · more beyond" : ""}`;
  return `${String(r.shape_label || "").replace(/-shaped$/, "")}-shaped · ${weightWord(r.weighting)} · ${plural((r.hits || []).length, "hit")} · ${n((r.micros || 0) / 1000)} ms`;
}

function weightWord(w) {
  if (!w) return "vector and keyword weighted equally";
  if (typeof w === "string") return w;
  return w.keyword > w.vector ? "keyword weighted ×2" : "vector and keyword weighted equally";
}

async function runSearch() {
  const q = sr.query.trim();
  if (!q) return;
  sr.busy = true;
  sr.error = "";
  sr.sel = null;
  sr.whole = false;
  sr.detail = null;
  state.recents = [q, ...state.recents.filter((x) => x !== q)].slice(0, 6);
  try {
    sessionStorage.setItem("semlith-recents", JSON.stringify(state.recents));
  } catch (_) {
    /* private window */
  }
  searchView.paintBody && searchView.paintBody();
  const p = new URLSearchParams();
  if (sr.store) p.append("store", sr.store);
  if (sr.lang) p.append("lang", sr.lang);
  if (sr.path) p.append("path", sr.path);
  try {
    if (sr.mode === "brief") {
      p.set("question", q);
      p.set("budget", String(sr.budget));
      if (sr.prefer !== "either") p.set("prefer", sr.prefer);
      sr.result = await api(`/api/brief?${p}`);
    } else if (sr.mode === "exact") {
      p.set("query", q);
      p.set("exact", "1");
      sr.result = await api(`/api/search?${p}`);
    } else if (sr.mode === "pattern") {
      if (!sr.patternLang) throw new Error("Pick the language the pattern is written for.");
      p.set("query", q);
      p.set("lang", sr.patternLang);
      sr.result = await api(`/api/pattern?${p}`);
      if (sr.result.error) throw new Error(sr.result.error);
    } else {
      p.set("query", q);
      p.set("k", String(sr.k));
      p.set("format", "locate");
      p.set("max_tokens", String(sr.budget));
      if (sr.prefer !== "either") p.set("prefer", sr.prefer);
      sr.result = await api(`/api/search?${p}`);
      // An identifier is answered by its definition first; every line that
      // names it is one press away, in Exact.
      if (sr.result.shape_label === "identifier" && !(sr.result.hits || []).length) {
        sr.mode = "exact";
        return runSearch();
      }
      const first = (sr.result.hits || [])[0];
      if (first) openHit(first);
    }
  } catch (e) {
    sr.result = { error: e.message };
  }
  sr.busy = false;
  if (current.view === VIEWS.search) repaint();
}

function searchBody() {
  if (sr.busy) return el("div", { class: "sr-body" }, el("div", { class: "row gap10 muted" }, el("span", { class: "spinner" }), "Searching…"));
  if (sr.result && sr.result.error) return el("div", { class: "sr-body" }, el("div", { class: "card" }, errorBox(sr.result.error)));
  if (!sr.result) return searchEmpty();
  if (sr.mode === "brief") return briefView(sr.result);
  if (sr.mode === "exact" || sr.mode === "pattern") return linesView(sr.result);
  return rankedView(sr.result);
}

function searchEmpty() {
  const recentQueries = (data.ledger?.rows || []).filter((r) => r.client !== "portal").map((r) => r.query);
  const ident = recentQueries.find((q) => /^[A-Za-z_][\w:]*$/.test(q) && q.length > 3);
  const examples = [
    ["QUESTION", "how does the daemon answer a request", "Ranked by meaning and words together", "ranked"],
    ["IDENTIFIER", ident || "main", "The definition first; every line in Exact", "ranked"],
    ["HOW-TO", "how do I add a new store", "Leans to the docs that explain it", "ranked"],
    ["BEHAVIOUR", "what keeps the index fresh when a file is saved", "Code and the design doc side by side", "brief"],
  ];
  return el(
    "div",
    { class: "sr-body" },
    el(
      "div",
      { class: "split s-15-1 max1200" },
      el(
        "div",
        { class: "card" },
        el("div", { class: "card-h" }, el("span", { class: "card-t", text: "Try one" })),
        el("div", { class: "auto-fit m240 gap0" }, examples.map(([kind, q, why, mode]) => btn({ class: "example", onclick: () => ((sr.query = q), (sr.mode = mode), repaint(), runSearch()) }, el("span", { class: "eyebrow sm", text: kind }), el("span", { class: "q", text: q }), el("span", { class: "w", text: why })))),
      ),
      el(
        "div",
        { class: "stack" },
        el(
          "div",
          { class: "card" },
          el("div", { class: "card-h" }, el("span", { class: "card-t", text: "Recent" })),
          state.recents.length ? state.recents.map((q) => btn({ class: "recent-row", onclick: () => ((sr.query = q), repaint(), runSearch()) }, icon(I.clock, 13, { w: 1.8 }), el("span", { class: "grow ell", text: q }), el("span", { class: "t-mono-sm", text: "this session" }))) : empty("Nothing searched in this tab yet."),
        ),
        el(
          "div",
          { class: "blue-box" },
          el("span", { class: "eyebrow", text: "FOUR MODES, ONE INDEX" }),
          [
            ["Ranked", "meaning and exact words fused, with graph neighbours mixed in"],
            ["Brief", "the best span in full plus a line per other hit — an agent's answer"],
            ["Exact", "every line that matches, for identifiers and error strings"],
            ["Pattern", "a tree-sitter query, for a shape of code rather than a word"],
          ].map(([k, v]) => el("div", { class: "txt" }, el("b", { text: k }), ` — ${v}`)),
        ),
      ),
    ),
  );
}

function rankedView(r) {
  const hits = r.hits || [];
  if (!hits.length)
    return el(
      "div",
      { class: "sr-body" },
      el("div", { class: "card" }, empty(r.selected === 0 ? "The filters select no file. Loosen the language or path." : "Nothing matched. Try other words, a wider scope, or Exact for a literal string.", "lg")),
    );
  const groups = [];
  for (const [i, h] of hits.entries()) {
    const key = `${h.store || ""}\n${h.path}`;
    let g = groups.find((x) => x.key === key);
    if (!g) groups.push((g = { key, path: h.path, store: h.store, remote: h.remote ? h : null, spans: [] }));
    g.spans.push({ ...h, i });
  }
  const tokens = r.tokens || 0;
  const pctUsed = Math.min(100, (tokens / sr.budget) * 100);
  const used = bar(pctUsed, `h5 grow ${pctUsed > 90 ? "accent" : "green"}`);
  used.classList.add("minw60");
  used.setAttribute("data-tip", "Token budget");
  used.setAttribute("data-tip-rows", rows([["sent", `${n(tokens)} tokens`], ["budget", n(sr.budget)], ["headroom", n(Math.max(0, sr.budget - tokens))], ["shown", r.truncated ? `${r.truncated.shown} of ${r.truncated.total}` : `${hits.length} of ${hits.length}`]]));
  return el(
    "div",
    { class: "sr-split" },
    el(
      "div",
      { class: "sr-left" },
      el(
        "div",
        { class: "sr-list", "data-scroll-keep": "sr-list" },
        unreadable(r.failed),
        r.remote_skipped ? el("div", { class: "notice amber" }, el("span", { class: "sub", text: `${r.remote_skipped}. The local stores answered in full.` })) : null,
        (r.pending || []).length ? el("div", { class: "notice amber" }, el("span", { class: "sub", text: `Still embedding: ${r.pending.map((p) => `${p.store} ${Math.round(p.share * 100)}% not yet ranked by meaning`).join(", ")}. Keyword and graph already cover it.` })) : null,
        groups.map((g) =>
          el(
            "div",
            { class: "card hit-group" },
            el(
              "div",
              { class: "hit-head" },
              pathSpan(hitPath(g), "p", g.path),
              el("span", { class: "t-mono-sm grow", text: g.store || "" }),
              g.remote ? pill(g.remote.badge, "blue", { dot: false, sm: true, tip: `${g.remote.revision ? `indexed at ${g.remote.revision}` : "revision not stated"} · ${lagWord(g.remote.behind_seconds)}` }) : null,
              el("span", { class: "t-mono-sm nowrap", text: g.spans.length > 1 ? `${g.spans.length} spans` : `${g.spans[0].start_line}-${g.spans[0].end_line}` }),
            ),
            g.spans.map((h) =>
              btn(
                { class: `hit${sr.sel === h.i ? " on" : ""}`, onclick: () => openHit(h) },
                el(
                  "div",
                  { class: "row gap6" },
                  el("span", { class: "rg", text: `${h.start_line}-${h.end_line}` }),
                  el("span", { class: "sym", text: h.symbol ? `${h.symbol_kind ? `${h.symbol_kind} ` : ""}${h.symbol}` : "" }),
                  el("span", { class: "spacer" }),
                  listBadges(h.lists),
                  h.fresh === false ? el("span", { class: "dot amber", "data-tip": "changed since it was indexed — read it before quoting it" }) : el("span", { class: "dot green", "data-tip": "unchanged since it was indexed" }),
                ),
                el("div", { class: "snip", text: h.line || h.text || "" }),
              ),
            ),
          ),
        ),
      ),
      el("div", { class: "sr-foot" }, el("span", { class: "nowrap", text: `${r.truncated ? `${r.truncated.shown} of ${r.truncated.total}` : `${hits.length} of ${hits.length}`} shown · ${n(tokens)} tokens` }), used, el("span", { class: "nowrap", text: `${Math.round(pctUsed)}% of ${n(sr.budget)} budget` })),
    ),
    el("div", { class: "sr-detail", "data-scroll-keep": "sr-detail" }, detailPanel()),
  );
}

function hitPath(h) {
  const s = h.store && store(h.store);
  return s ? relTo(s, h.path) : tilde(h.path);
}

function unreadable(failed) {
  if (!failed || !failed.length) return null;
  return el("div", { class: "notice red" }, el("div", { class: "body" }, el("span", { class: "ttl", text: `${plural(failed.length, "store")} could not be read, so ${failed.length > 1 ? "they are" : "it is"} not in these results` }), el("span", { class: "sub", text: failed.map((f) => `${f.store}: ${f.remedy || f.error}`).join(" · ") })));
}

async function openHit(h) {
  sr.sel = h.i;
  sr.whole = false;
  sr.detail = { hit: h, span: null, hops: null };
  // A remote row arrived with its text: shown from that, with no second
  // request to the host.
  if (h.remote) {
    sr.detail.span = { text: h.text || h.line || "", start_line: h.start_line, end_line: h.end_line, fresh: h.fresh };
    if (current.view === VIEWS.search) searchView.paintBody();
    return;
  }
  if (current.view === VIEWS.search) searchView.paintBody();
  const p = new URLSearchParams({ target: `${h.path}:${h.start_line}-${h.end_line}` });
  if (h.store) p.append("store", h.store);
  try {
    const out = await api(`/api/read?${p}`);
    if (sr.detail && sr.detail.hit === h) sr.detail.span = out.span;
  } catch (e) {
    if (sr.detail && sr.detail.hit === h) sr.detail.error = e.message;
  }
  if (h.symbol) {
    const q = new URLSearchParams({ name: h.symbol, k: "6" });
    if (h.store) q.append("store", h.store);
    try {
      const ev = await api(`/api/symbol?${q}`);
      if (sr.detail && sr.detail.hit === h) sr.detail.hops = ev;
    } catch (_) {
      if (sr.detail && sr.detail.hit === h) sr.detail.hops = { callers: [], callees: [] };
    }
  }
  if (current.view === VIEWS.search) searchView.paintBody();
}

async function readWhole() {
  const d = sr.detail;
  if (!d || !d.hit.symbol) return;
  if (sr.whole) {
    sr.whole = false;
    d.whole = null;
    return searchView.paintBody();
  }
  const p = new URLSearchParams({ target: d.hit.symbol });
  if (d.hit.store) p.append("store", d.hit.store);
  try {
    const out = await api(`/api/read?${p}`);
    if (out.span) {
      d.whole = out.span;
      sr.whole = true;
    } else if ((out.definitions || []).length) {
      const def = out.definitions.find((x) => x.path === d.hit.path) || out.definitions[0];
      const again = await api(`/api/read?${new URLSearchParams({ target: `${def.path}:${def.start_line}-${def.end_line}`, ...(d.hit.store ? { store: d.hit.store } : {}) })}`);
      d.whole = again.span;
      sr.whole = !!again.span;
    }
  } catch (e) {
    toast(e.message, true);
  }
  searchView.paintBody();
}

function detailPanel() {
  const d = sr.detail;
  if (!d) return empty("Pick a result to read it here.");
  const h = d.hit;
  const span = sr.whole && d.whole ? d.whole : d.span;
  const ref = `${hitPath(h)}:${span ? span.start_line : h.start_line}-${span ? span.end_line : h.end_line}`;
  const fresh = span ? span.fresh !== false : h.fresh !== false;
  const lines = span ? span.text.split("\n").map((t, i) => el("div", { class: "l" }, el("span", { class: "n", text: String(span.start_line + i) }), el("span", { text: t }))) : null;
  const hops = d.hops ? [...(d.hops.callers || []).map((c) => ["CALLED BY", c]), ...(d.hops.callees || []).map((c) => ["CALLS", c])].slice(0, 8) : null;
  return el(
    "div",
    { class: "stack gap10" },
    el(
      "div",
      { class: "card" },
      el(
        "div",
        { class: "card-b line-row gap8" },
        el("div", { class: "row nowrap" }, pathSpan(hitPath(h), "mono t-b t-sm", h.path), el("span", { class: "t-mono-sm grow", text: span ? `${span.start_line}-${span.end_line}` : `${h.start_line}-${h.end_line}` }), pill(fresh ? (span && span.from_disk ? "read from disk" : "fresh") : "stale", fresh ? "green" : "amber")),
        el("div", { class: "title-strip", text: h.symbol ? `${h.symbol_kind || ""} ${h.symbol}`.trim() : h.line || h.path }),
      ),
      d.error ? errorBox(d.error) : lines ? el("div", { class: "lines scroll" }, lines) : el("div", { class: "empty" }, "Reading…"),
      el(
        "div",
        { class: "card-foot sans" },
        h.symbol ? btn({ class: "btn sm dark t125", onclick: readWhole }, sr.whole ? "Show the span only" : "Read whole symbol") : null,
        btn({ class: "btn sm t125", onclick: () => copy(ref, `Copied ${ref}`) }, "Copy path:lines"),
        el("span", { class: "spacer" }),
        h.symbol ? lnk("Open in graph →", () => ((state.pending.graph = { store: h.store, name: h.symbol }), go("graph"))) : null,
      ),
    ),
    h.symbol
      ? el(
          "div",
          { class: "card" },
          el("div", { class: "card-h tight" }, el("span", { class: "card-t sm", text: "One hop around" }), el("span", { class: "mono t-m t-sm grow", text: h.symbol }), el("span", { class: "t-mono-sm", text: "what an agent's brief would add" })),
          !hops ? el("div", { class: "empty" }, "Reading…") : !hops.length ? empty("No call edges in or out of this symbol.") : null,
          (hops || []).map(([dir, c]) =>
            btn(
              { class: "hop-row", onclick: () => c.confidence !== "ambiguous" && jumpTo(c, h.store) },
              el("span", { class: "dir", text: dir }),
              el("span", { class: "row nowrap gap8 min0" }, el("span", { class: "mono t-m t-sm", text: c.name }), el("span", { class: "t-mono-sm ell", text: c.confidence === "ambiguous" ? `${c.definitions} definitions` : `${tilde(c.path)}:${c.start_line}` })),
              confBadge(c.confidence),
            ),
          ),
        )
      : null,
  );
}

function jumpTo(c, storeName) {
  sr.query = c.name;
  sr.mode = "ranked";
  if (storeName) sr.store = storeName;
  repaint();
  runSearch();
}

function confBadge(c) {
  const tone = c === "resolved" || c === "extracted" ? "blue" : c === "ambiguous" ? "amber" : "";
  return el("span", { class: `badge ${tone}`, text: c || "" });
}

function briefView(out) {
  const b = out.brief || {};
  const spans = b.spans || [];
  const top = spans[0];
  const rest = spans.slice(1);
  const agentText = out.text || briefText(b);
  return el(
    "div",
    { class: "sr-body tight" },
    el(
      "div",
      { class: "stack gap10 max980" },
      el("div", { class: "notice plain" }, el("span", { class: "eyebrow sm wide blue-ink", text: "BRIEF" }), "This is exactly what semlith_brief hands an agent: the best span in full, then one line per other hit, inside the budget."),
      unreadable(out.failed),
      !top ? el("div", { class: "card" }, empty("Nothing matched inside the budget.", "lg")) : null,
      top
        ? el(
            "div",
            { class: "card" },
            el("div", { class: "card-h tight" }, pathSpan(hitPath(top), "mono t-b t-sm min0", top.path), el("span", { class: "t-mono-sm", text: `${top.start_line}-${top.end_line}` }), el("span", { class: "t-mono-sm grow", text: top.store || "" }), listBadges(top.lists), el("span", { class: `dot ${top.fresh === false ? "amber" : "green"}` })),
            top.text ? el("div", { class: "lines wrap" }, top.text.split("\n").map((t, i) => el("div", { class: "l" }, el("span", { class: "n", text: String(top.start_line + i) }), el("span", { text: t })))) : empty("The budget left no room for the text; the locator is still sent."),
          )
        : null,
      rest.length
        ? el(
            "div",
            { class: "card" },
            rest.map((s) => el("div", { class: "brief-row" }, pathSpan(`${hitPath(s)}:${s.start_line}-${s.end_line}`, "mono t-sm ink2 min0", `${s.path}:${s.start_line}-${s.end_line}`), el("span", { class: "mono t-m t-sm grow", text: s.symbol || "" }), el("span", { class: "t-mono-sm muted i", text: s.text ? "with text" : "top span only" }), listBadges(s.lists))),
          )
        : null,
      (b.symbols || []).length
        ? el(
            "div",
            { class: "card" },
            el("div", { class: "card-h tight" }, el("span", { class: "card-t sm", text: "Edges the brief carries" })),
            b.symbols.slice(0, 6).map((sym) => el("div", { class: "brief-row" }, el("span", { class: "mono t-m t-sm grow", text: sym.name }), el("span", { class: "t-mono-sm", text: `${plural((sym.callers || []).length, "caller")} · ${plural((sym.callees || []).length, "callee")}` }))),
          )
        : null,
      el(
        "div",
        { class: "card pad row" },
        el("span", { class: "t-mono-sm" }, "tokens ", el("span", { class: "ink", text: `${n(b.tokens || 0)} of ${n(b.budget || sr.budget)}` })),
        el("span", { class: "t-mono-sm" }, "spans ", el("span", { class: "ink", text: String(spans.length) })),
        el("span", { class: "t-mono-sm" }, "dropped ", el("span", { class: "ink", text: cutWord(b.cut) })),
        el("span", { class: "spacer" }),
        lnk("Copy as the agent sees it", () => copy(agentText, "Brief copied as plain text")),
      ),
    ),
  );
}

function cutWord(cut) {
  if (!cut) return "nothing";
  const parts = [];
  if (cut.spans) parts.push(plural(cut.spans, "span"));
  if (cut.span_text) parts.push(`${plural(cut.span_text, "span")}' text`);
  if (cut.symbols) parts.push(plural(cut.symbols, "symbol"));
  if (cut.edges) parts.push(plural(cut.edges, "edge"));
  return parts.length ? parts.join(", ") : "nothing";
}

/** Only for a daemon that does not send the agent's own text. */
function briefText(b) {
  return (b.spans || []).map((s) => `${s.path}:${s.start_line}-${s.end_line}${s.symbol ? ` ${s.symbol}` : ""}${s.text ? `\n${s.text}` : ""}`).join("\n\n");
}

function linesView(r) {
  const matches = r.matches || [];
  if (!matches.length) return el("div", { class: "sr-body" }, el("div", { class: "card" }, empty(sr.mode === "pattern" ? "The pattern matched nothing in that language's files." : "No indexed line matches.", "lg")));
  const groups = [];
  for (const [i, m] of matches.entries()) {
    const key = `${m.store || ""}\n${m.path}`;
    let g = groups.find((x) => x.key === key);
    if (!g) groups.push((g = { key, path: m.path, store: m.store, spans: [] }));
    g.spans.push({ ...m, i });
  }
  return el(
    "div",
    { class: "sr-split" },
    el(
      "div",
      { class: "sr-left" },
      el(
        "div",
        { class: "sr-list", "data-scroll-keep": "sr-list" },
        groups.map((g) =>
          el(
            "div",
            { class: "card hit-group" },
            el("div", { class: "hit-head" }, pathSpan(hitPath(g), "p", g.path), el("span", { class: "t-mono-sm grow", text: g.store || "" }), el("span", { class: "t-mono-sm", text: plural(g.spans.length, "line") })),
            g.spans.map((m) =>
              btn(
                { class: `hit${sr.sel === m.i ? " on" : ""}`, onclick: () => openHit({ ...m, path: m.path, start_line: m.start_line, end_line: m.end_line || m.start_line, store: m.store, symbol: null, line: m.text }) },
                el("div", { class: "row gap6" }, el("span", { class: "rg", text: m.end_line && m.end_line !== m.start_line ? `${m.start_line}-${m.end_line}` : String(m.start_line) }), m.capture ? el("span", { class: "badge green", text: `@${m.capture}` }) : null),
                el("div", { class: "snip", text: (m.text || "").split("\n")[0] }),
              ),
            ),
          ),
        ),
        r.truncated ? el("div", { class: "muted t-sm", text: "More lines match than are listed. Narrow the scope or the path to see the rest." }) : null,
      ),
      el("div", { class: "sr-foot" }, el("span", { text: `${plural(matches.length, "line")} in ${plural(r.files || groups.length, "file")} · ${sr.mode === "pattern" ? `tree-sitter · ${sr.patternLang}` : "exact lines, as grep -E"}` })),
    ),
    el("div", { class: "sr-detail", "data-scroll-keep": "sr-detail" }, detailPanel()),
  );
}

// --------------------------------------------------------------------- graph

/* One store at a time, picked in the toolbar: the graph of every store at once
 * is a hairball on a small corpus and wedged the daemon on the 879k one. */
const gr = { store: "", sel: "", find: "", off: {}, dir: "in", unres: false, mapOpen: true, data: null, sym: null, map: null, br: { sym: "", hops: 3, verified: true, q: "", hop: "all", edge: "all", out: null }, pt: { from: "", to: "", verified: true, strict: false, out: null, showInferred: false } };

let graphDragging = false;

// The store the Graph page shows: the one picked, or the first with a graph.
let graphMapFor = "";
let graphPeekFor = "";
function graphStore() {
  const stores = liveStores().filter((s) => s.files);
  if (gr.store && store(gr.store)) return gr.store;
  return (stores.find((s) => s.files && (s.coverage || []).length) || stores[0] || {}).name || "";
}

VIEWS.graph = {
  fill: true,
  // Blast radius opens on the store's hubs, so they load with the page.
  needs: (route) => (route.parts[0] === "blast" ? ["stores", "graphmap", "graphpeek"] : ["stores"]),
  live: [],
  render(route) {
    const tab = route.parts[0] || "explore";
    const stores = liveStores().filter((s) => s.files);
    if (state.pending.graph) {
      const p = state.pending.graph;
      delete state.pending.graph;
      if (p.store && store(p.store)) gr.store = p.store;
      gr.sel = p.name || "";
      gr.data = null;
      gr.sym = null;
    }
    if (!gr.store || !store(gr.store)) gr.store = graphStore();
    if (data.graphmap && graphMapFor === gr.store && !(gr.map && gr.map.store === gr.store)) gr.map = { ...data.graphmap, store: gr.store };
    const root = el("div", { class: "col fillpage grow min0" });
    root.style.height = "100%";
    const picker = btn(
      { class: "btn", "aria-haspopup": "menu", "data-tip": "The graph shows one store at a time", onclick: () => menu.open(picker, stores.map((s) => ({ label: s.name, hint: `${n(s.files)} files`, checked: gr.store === s.name, onclick: () => ((gr.store = s.name), (gr.sel = ""), (gr.data = null), (gr.sym = null), (gr.map = null), (gr.br.out = null), (gr.pt.out = null), repaint()) })), { width: 220 }) },
      el("span", { class: "muted", text: "in" }),
      el("span", { class: "mono t-m", text: gr.store || "no store" }),
      icon(I.chevDown, 12, { w: 2 }),
    );
    root.append(
      el(
        "div",
        { class: "gr-head" },
        el("div", { class: "row base gap12" }, el("div", { class: "h1", text: "Graph" }), el("div", { class: "muted t-sm grow", text: "Who calls what, re-extracted on the same pass that re-embeds a file. Every edge says how sure it is." })),
        tabs(
          [
            ["explore", "Explore"],
            ["blast", "Blast radius"],
            ["path", "Path & evidence"],
          ],
          tab,
          (t) => go("graph", t === "explore" ? undefined : t),
          { cls: "bare" },
        ),
      ),
    );
    if (!stores.length) {
      root.append(el("div", { class: "gr-pad" }, el("div", { class: "card" }, empty("No store has anything indexed yet, so there is no graph to draw.", "lg"))));
      return root;
    }
    root.append(tab === "blast" ? blastTab(picker) : tab === "path" ? pathTab(picker) : exploreTab(picker));
    return root;
  },
};

const EDGE_INK = { solid: "#4C7088", inferred: "#A9B8C4", ambiguous: "#B07A2A", other: "#D5DDE3" };

function edgeStyle(e, darkMuted) {
  if (e.confidence === "ambiguous") return { color: EDGE_INK.ambiguous, dash: "4 3", w: 1 };
  if (e.confidence === "inferred") return { color: darkMuted ? "#5A6B78" : EDGE_INK.inferred, dash: "2 2", w: 1 };
  if (e.kind && e.kind !== "calls") return { color: isDark() ? "#3E4E5B" : EDGE_INK.other, dash: "0", w: 1 };
  return { color: isDark() ? "#86AEC7" : EDGE_INK.solid, dash: "0", w: 1.4 };
}

/** A quick force layout in 0..100 space, so the live canvas starts settled. */
function layout(nodes, edges, centre) {
  const pos = nodes.map((nd, i) => {
    const a = i * 2.39996 + 0.4;
    const r = 0.25 + ((i * 37) % 100) / 140;
    return { x: 50 + Math.cos(a) * r * 40, y: 50 + Math.sin(a) * r * 40, vx: 0, vy: 0 };
  });
  if (centre !== undefined && pos[centre]) Object.assign(pos[centre], { x: 50, y: 50 });
  for (let it = 0; it < 220; it++) {
    for (let i = 0; i < pos.length; i++)
      for (let j = i + 1; j < pos.length; j++) {
        const dx = pos[j].x - pos[i].x;
        const dy = (pos[j].y - pos[i].y) * 1.6;
        const d2 = Math.max(4, dx * dx + dy * dy);
        const f = 60 / d2;
        pos[i].vx -= dx * f;
        pos[i].vy -= dy * f;
        pos[j].vx += dx * f;
        pos[j].vy += dy * f;
      }
    for (const e of edges) {
      const a = pos[e.from];
      const b = pos[e.to];
      if (!a || !b) continue;
      const dx = b.x - a.x;
      const dy = b.y - a.y;
      const d = Math.sqrt(dx * dx + dy * dy) || 1;
      const f = (d - 18) * 0.02;
      a.vx += (dx / d) * f;
      a.vy += (dy / d) * f;
      b.vx -= (dx / d) * f;
      b.vy -= (dy / d) * f;
    }
    for (const [i, p] of pos.entries()) {
      p.vx += (50 - p.x) * 0.01;
      p.vy += (50 - p.y) * 0.01;
      if (i === centre) {
        p.vx = 0;
        p.vy = 0;
        continue;
      }
      p.x = Math.max(6, Math.min(94, p.x + p.vx * 0.5));
      p.y = Math.max(7, Math.min(93, p.y + p.vy * 0.5));
      p.vx *= 0.6;
      p.vy *= 0.6;
    }
  }
  return pos;
}

/* The live canvas: DOM labels over an SVG of lines, with hover dimming the
 * edges that do not touch it. Positions go through the CSSOM.
 *
 * The motion is the force layout the portal had before 0.35.0, kept on
 * purpose: every node pushes every other away, an edge pulls its two ends to
 * a resting length, the layout cools from hot to still, and a slow sine drift
 * keeps it alive. Dragging a node re-heats it, so its neighbours follow and
 * the rest makes room. Positions are percentages of the host; the forces are
 * worked in pixels, which is the unit their constants were tuned in. */
const G_MAX_STEP = 1.2; // % of the host a node may move in one frame
const G_DRIFT = 0.003; // % per frame, the wander's push
const G_REST = 138; // px, the length an edge relaxes to

function makeLive(host) {
  const g = { nodes: {}, raf: 0, drag: null, hover: null, tick: 0, alpha: 1, settle: 0 };
  const still = stillness();
  const pt = (e) => {
    const r = host.getBoundingClientRect();
    return [((e.clientX - r.left) / r.width) * 100, ((e.clientY - r.top) / r.height) * 100];
  };
  const nodeOf = (t) => {
    const ne = t && t.closest ? t.closest("[data-gn]") : null;
    return ne && host.contains(ne) ? g.nodes[ne.getAttribute("data-gn")] || null : null;
  };
  const down = (e) => {
    if (e.button !== 0) return;
    const nd = nodeOf(e.target);
    if (!nd) return;
    const p = pt(e);
    g.drag = { nd, dx: nd.x - p[0], dy: nd.y - p[1], sx: e.clientX, sy: e.clientY, moved: false };
    g.alpha = Math.max(g.alpha, 0.35);
    graphDragging = true;
    nd.el.style.cursor = "grabbing";
    e.preventDefault();
  };
  const move = (e) => {
    const d = g.drag;
    if (!d) return;
    if (Math.abs(e.clientX - d.sx) + Math.abs(e.clientY - d.sy) > 3) d.moved = true;
    const p = pt(e);
    d.nd.x = Math.max(3, Math.min(97, p[0] + d.dx));
    d.nd.y = Math.max(4, Math.min(96, p[1] + d.dy));
    d.nd.vx = 0;
    d.nd.vy = 0;
    g.alpha = Math.max(g.alpha, 0.2);
  };
  const up = () => {
    const d = g.drag;
    if (!d) return;
    g.suppress = d.moved;
    if (d.nd.el) d.nd.el.style.cursor = "grab";
    g.drag = null;
    graphDragging = false;
    setTimeout(() => (g.suppress = false), 0);
  };
  const clickCap = (e) => {
    if (g.suppress) {
      e.stopPropagation();
      e.preventDefault();
      g.suppress = false;
    }
  };
  host.addEventListener("pointerdown", down);
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", up);
  host.addEventListener("click", clickCap, true);
  host.addEventListener("pointerover", (e) => (g.hover = nodeOf(e.target)));
  host.addEventListener("pointerleave", () => (g.hover = null));
  const stop = () => {
    cancelAnimationFrame(g.raf);
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", up);
  };
  // Fit: every node back to its laid-out place, and the heat back in so the
  // layout settles from there in view.
  g.reset = () => {
    for (const nd of Object.values(g.nodes)) {
      nd.x = nd.ox;
      nd.y = nd.oy;
      nd.vx = 0;
      nd.vy = 0;
    }
    g.alpha = 0.6;
  };
  // One frame of the simulation. `arr` are the nodes, `links` the edges.
  const step = (arr, links, W, H, t, dn) => {
    const a0 = g.alpha;
    for (let i = 0; i < arr.length; i++) {
      const a = arr[i];
      for (let j = i + 1; j < arr.length; j++) {
        const b = arr[j];
        let dx = ((b.x - a.x) * W) / 100;
        let dy = ((b.y - a.y) * H) / 100;
        let d2 = dx * dx + dy * dy;
        if (d2 < 1) {
          // Coincident nodes have no direction to separate along.
          d2 = 1;
          dx = 0.6;
          dy = 0.4;
        }
        const d = Math.sqrt(d2);
        const f = (5200 / d2) * a0;
        a.vx -= ((dx / d) * f * 100) / W;
        a.vy -= ((dy / d) * f * 100) / H;
        b.vx += ((dx / d) * f * 100) / W;
        b.vy += ((dy / d) * f * 100) / H;
      }
    }
    for (const [a, b] of links) {
      const dx = ((b.x - a.x) * W) / 100;
      const dy = ((b.y - a.y) * H) / 100;
      const d = Math.max(1, Math.sqrt(dx * dx + dy * dy));
      const pull = (d - G_REST) * 0.0025 * a0 * 0.02;
      // Shared out by degree at each end, so a hub is not dragged about by
      // every edge it carries.
      const ka = pull / Math.sqrt(a.deg || 1);
      const kb = pull / Math.sqrt(b.deg || 1);
      a.vx += (dx * ka * 100) / W;
      a.vy += (dy * ka * 100) / H;
      b.vx -= (dx * kb * 100) / W;
      b.vy -= (dy * kb * 100) / H;
    }
    const drift = still ? 0 : G_DRIFT;
    const clock = t * 0.28;
    for (const nd of arr) {
      nd.vx += (50 - nd.x) * 0.006 * a0;
      nd.vy += (50 - nd.y) * 0.008 * a0;
      nd.vx += Math.cos(clock + nd.ph) * drift;
      nd.vy += Math.sin(clock * 0.9 + nd.ph * 1.7) * drift;
    }
    // Labels are boxes, not points: two that overlap are nudged apart along
    // whichever axis needs the smaller move.
    for (let i = 0; i < arr.length; i++)
      for (let j = i + 1; j < arr.length; j++) {
        const a = arr[i];
        const b = arr[j];
        const dx = ((b.x - a.x) * W) / 100;
        const dy = ((b.y - a.y) * H) / 100;
        const mx = (a.w + b.w) / 2 + 6 - Math.abs(dx);
        const my = (a.h + b.h) / 2 + 5 - Math.abs(dy);
        if (mx > 0 && my > 0) {
          if (mx < my) {
            const f = (mx * 0.04 * (dx < 0 ? -1 : 1) * 100) / W;
            a.vx -= f;
            b.vx += f;
          } else {
            const f = (my * 0.04 * (dy < 0 ? -1 : 1) * 100) / H;
            a.vy -= f;
            b.vy += f;
          }
        }
      }
    for (const nd of arr) {
      if (nd === dn) {
        nd.vx = 0;
        nd.vy = 0;
        continue;
      }
      nd.vx = Math.max(-G_MAX_STEP, Math.min(G_MAX_STEP, nd.vx * 0.86));
      nd.vy = Math.max(-G_MAX_STEP, Math.min(G_MAX_STEP, nd.vy * 0.86));
      // Kept inside the frame by half its own size, so no label is cut.
      const mx = Math.min(45, (((nd.w || 80) / 2 + 4) / W) * 100);
      const my = Math.min(45, (((nd.h || 28) / 2 + 4) / H) * 100);
      nd.x = Math.max(mx, Math.min(100 - mx, nd.x + nd.vx));
      nd.y = Math.max(my, Math.min(100 - my, nd.y + nd.vy));
    }
    g.alpha = Math.max(0, g.alpha * 0.985);
  };
  const frame = (now) => {
    if (!host.isConnected) return stop();
    g.raf = requestAnimationFrame(frame);
    if (document.visibilityState === "hidden") return;
    const W = host.clientWidth || 1;
    const H = host.clientHeight || 1;
    const t = now / 1000;
    const remeasure = g.tick++ % 30 === 0;
    const arr = [];
    const seen = {};
    host.querySelectorAll("[data-gn]").forEach((ne) => {
      const id = ne.getAttribute("data-gn");
      const ox = parseFloat(ne.getAttribute("data-gx"));
      const oy = parseFloat(ne.getAttribute("data-gy"));
      let nd = g.nodes[id];
      if (!nd) {
        // A node that appears starts where the layout put it, and the
        // simulation warms up to settle the new shape.
        nd = g.nodes[id] = { x: ox, y: oy, vx: 0, vy: 0, ox, oy, ph: Math.random() * 6.283, hv: 0, deg: 0 };
        g.settle = 1;
        g.alpha = 1;
      }
      if (nd.el !== ne || remeasure) {
        nd.w = ne.offsetWidth;
        nd.h = ne.offsetHeight;
      }
      nd.el = ne;
      seen[id] = nd;
      arr.push(nd);
    });
    const links = [];
    host.querySelectorAll("[data-ge]").forEach((le) => {
      const k = (le.getAttribute("data-ge") || "").split("|");
      const a = seen[k[0]];
      const b = seen[k[1]];
      if (a && b) links.push([a, b, le]);
    });
    const dn = g.drag ? g.drag.nd : null;
    for (const nd of arr) nd.deg = 0;
    for (const [a, b] of links) {
      a.deg++;
      b.deg++;
    }
    // A new shape is settled before it is shown moving, as the old layout
    // was: two hundred quiet steps, then the drift from there.
    if (g.settle && arr.every((nd) => nd.w)) {
      for (let i = 0; i < 220; i++) step(arr, links, W, H, t, dn);
      g.settle = 0;
    }
    step(arr, links, W, H, t, dn);
    for (const nd of arr) {
      nd.hv += ((g.hover === nd || nd === dn ? 1 : 0) - nd.hv) * 0.2;
      if (nd.hv < 0.003) nd.hv = 0;
      const s = nd.el.style;
      s.left = `${nd.x.toFixed(3)}%`;
      s.top = `${nd.y.toFixed(3)}%`;
      s.transform = `translate(-50%,-50%) scale(${(1 + 0.08 * nd.hv).toFixed(3)})`;
      s.boxShadow = nd.hv ? `0 4px 14px rgba(20,28,36,${(0.2 * nd.hv).toFixed(3)})` : "";
    }
    const hv = g.hover || dn;
    const off = -(t * 14) % 1000;
    for (const [a, b, le] of links) {
      le.setAttribute("x1", a.x.toFixed(3));
      le.setAttribute("y1", a.y.toFixed(3));
      le.setAttribute("x2", b.x.toFixed(3));
      le.setAttribute("y2", b.y.toFixed(3));
      le.style.strokeDashoffset = still ? "0" : off.toFixed(2);
      le.style.opacity = hv ? (a === hv || b === hv ? "1" : "0.18") : "1";
    }
  };
  g.raf = requestAnimationFrame(frame);
  return g;
}

/** Draw nodes and edges into a live host. `nodes` are `{ id, label, x, y, cls, tip, rows, color, onclick }`. */
function drawLive(host, nodes, edges) {
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("viewBox", "0 0 100 100");
  svg.setAttribute("preserveAspectRatio", "none");
  svg.setAttribute("class", "edges");
  for (const e of edges) {
    const a = nodes.find((x) => x.id === e.a);
    const b = nodes.find((x) => x.id === e.b);
    if (!a || !b) continue;
    const line = document.createElementNS(SVG_NS, "line");
    line.setAttribute("data-ge", `${e.a}|${e.b}`);
    line.setAttribute("x1", a.x);
    line.setAttribute("y1", a.y);
    line.setAttribute("x2", b.x);
    line.setAttribute("y2", b.y);
    line.setAttribute("stroke", e.color);
    line.setAttribute("stroke-width", String(e.w || 1.2));
    line.setAttribute("stroke-dasharray", e.dash || "0");
    line.setAttribute("vector-effect", "non-scaling-stroke");
    svg.append(line);
  }
  fill(
    host,
    svg,
    nodes.map((nd) => {
      const node = (nd.onclick ? btn : (p, ...k) => el("span", p, ...k))(
        {
          class: `g-node${nd.cls ? ` ${nd.cls}` : ""}`,
          "data-gn": nd.id,
          "data-gx": String(nd.x),
          "data-gy": String(nd.y),
          "data-tip": nd.label,
          "data-tip-rows": nd.rows || null,
          "data-tip-color": nd.color || null,
          onclick: nd.onclick || null,
        },
        nd.label,
      );
      node.style.left = `${nd.x}%`;
      node.style.top = `${nd.y}%`;
      return node;
    }),
  );
  if (!host._live) host._live = makeLive(host);
  return host._live;
}

// Several /api/graph answers as one drawing: nodes by name, edges re-pointed.
function mergeGraphs(parts) {
  if (parts.length === 1) return parts[0];
  const nodes = [];
  const at = new Map();
  const edges = [];
  const seen = new Set();
  let total = 0;
  for (const g of parts) {
    total = Math.max(total, g.total || 0);
    const local = (g.nodes || []).map((nd) => {
      if (!at.has(nd.name)) at.set(nd.name, nodes.push(nd) - 1);
      return at.get(nd.name);
    });
    for (const e of g.edges || []) {
      const from = local[e.from];
      const to = local[e.to];
      const key = `${from}>${to}>${e.kind}`;
      if (from === undefined || to === undefined || seen.has(key)) continue;
      seen.add(key);
      edges.push({ ...e, from, to });
    }
  }
  return { nodes, edges, total, shown: nodes.length };
}

function exploreTab(picker) {
  const wrap = el("div", { class: "gr-explore grow min0" });
  const map = el("div", { class: "g-map" });
  const live = el("div", { class: "g-live" });
  const side = el("div", { class: "g-side" });
  const foot = el("div", { class: "g-foot" });
  const findInput = el("input", {
    class: "mono",
    value: gr.find,
    placeholder: "Jump to a symbol",
    "aria-label": "Jump to a symbol",
    "data-keep": "gr-find",
    "aria-describedby": "gr-find-hint",
    oninput: (e) => (gr.find = e.target.value),
    onkeydown: (e) => {
      if (e.key !== "Enter") return;
      jump();
    },
  });
  // Several names, comma-separated: each one's definition, and the graph
  // around all of them, centred on the first.
  function jump() {
    const names = gr.find.split(",").map((x) => x.trim()).filter(Boolean).slice(0, 8);
    if (!names.length) return;
    gr.names = names;
    gr.sel = names[0];
    gr.data = null;
    gr.sym = null;
    gr.defs = null;
    loadExplore();
  }
  let liveCtl = null;
  map.append(
    el(
      "div",
      { class: "g-bar" },
      el(
        "div",
        { class: "box w260 full-sm" },
        icon(I.searchSm, 14, { w: 1.8 }),
        findInput,
        el("span", { class: "sr-only", id: "gr-find-hint", text: "Press Enter or Go to jump. Separate several names with commas." }),
        btn({ class: "x-btn", "aria-label": "Go", "data-tip": "Jump · Enter does the same", onclick: jump }, icon(I.arrow, 12, { w: 2 })),
      ),
      el(
        "div",
        { class: "seg panel" },
        [
          ["calls", "calls"],
          ["imports", "imports"],
          ["inferred", "inferred"],
          ["ambiguous", "ambiguous"],
        ].map(([k, label]) => btn({ "aria-pressed": String(!gr.off[k]), onclick: () => ((gr.off[k] = !gr.off[k]), paint()) }, label)),
      ),
      el("span", { class: "spacer" }),
      picker,
      btn({ class: "btn", onclick: () => liveCtl && liveCtl.reset() }, "Fit"),
    ),
    live,
    foot,
  );
  wrap.append(map, side);

  async function loadExplore() {
    fill(foot, el("span", { class: "muted", text: "Reading the graph…" }));
    const names = gr.names && gr.names.length > 1 && gr.names.includes(gr.sel) ? gr.names : gr.sel ? [gr.sel] : [null];
    try {
      const parts = await Promise.all(
        names.map((nm) => {
          const p = new URLSearchParams({ store: gr.store, limit: names.length > 1 ? "24" : "63" });
          if (nm) p.set("name", nm);
          return api(`/api/graph?${p}`);
        }),
      );
      gr.data = mergeGraphs(parts);
      if (names.length > 1) {
        api(`/api/symbol?${new URLSearchParams({ names: names.join(","), store: gr.store })}`)
          .then((out) => ((gr.defs = out.table || []), paintSide()))
          .catch(() => {});
      } else gr.defs = null;
      if (!gr.sel && gr.data.nodes && gr.data.nodes.length) {
        const deg = {};
        for (const e of gr.data.edges) {
          deg[e.from] = (deg[e.from] || 0) + 1;
          deg[e.to] = (deg[e.to] || 0) + 1;
        }
        const hub = Object.entries(deg).sort((a, b) => b[1] - a[1])[0];
        gr.sel = hub ? gr.data.nodes[hub[0]].name : gr.data.nodes[0].name;
      }
    } catch (e) {
      gr.data = { error: e.message, nodes: [], edges: [] };
    }
    paint();
    loadSymbol();
  }

  async function loadSymbol() {
    if (!gr.sel) return;
    const p = new URLSearchParams({ name: gr.sel, store: gr.store, k: "40" });
    try {
      gr.sym = await api(`/api/symbol?${p}`);
      if (gr.unres) {
        const nb = await api(`/api/neighbors?${new URLSearchParams({ name: gr.sel, store: gr.store, all: "1" })}`);
        gr.sym.unresolved = nb.unresolved || [];
      }
    } catch (e) {
      gr.sym = { error: e.message };
    }
    paintSide();
  }

  async function loadMap() {
    if (gr.map && gr.map.store === gr.store) return;
    try {
      const out = await api(`/api/map?${new URLSearchParams({ store: gr.store, shown: "12" })}`);
      gr.map = { ...out, store: gr.store };
    } catch (e) {
      gr.map = { error: e.message, store: gr.store, communities: [] };
    }
    paintSide();
  }

  function paint() {
    const d = gr.data;
    if (!d) return;
    if (d.error) {
      fill(live, errorBox(d.error));
      return;
    }
    // Every chip off means no edges at all, not the kinds no chip names.
    const allOff = gr.off.calls && gr.off.imports && gr.off.inferred && gr.off.ambiguous;
    const keep = d.edges.filter((e) => {
      if (allOff) return false;
      if (e.confidence === "inferred" && gr.off.inferred) return false;
      if (e.confidence === "ambiguous" && gr.off.ambiguous) return false;
      if (e.kind === "calls" && gr.off.calls && e.confidence !== "inferred" && e.confidence !== "ambiguous") return false;
      if (e.kind === "imports" && gr.off.imports) return false;
      return true;
    });
    const selIdx = d.nodes.findIndex((nd) => nd.name === gr.sel);
    const pos = layout(d.nodes, keep, selIdx >= 0 ? selIdx : undefined);
    const deg = {};
    for (const e of keep) {
      deg[e.from] = (deg[e.from] || 0) + 1;
      deg[e.to] = (deg[e.to] || 0) + 1;
    }
    const near = new Set(keep.filter((e) => e.from === selIdx || e.to === selIdx).flatMap((e) => [e.from, e.to]));
    // As many labels as the canvas has room for: sixty-three in a phone's
    // width were a pile. The selected symbol, its neighbours, then the best
    // connected, up to a count that grows with the width.
    const width = live.clientWidth || 900;
    const room = width < 520 ? 14 : width < 760 ? 24 : width < 1000 ? 40 : d.nodes.length;
    const shown = new Set(
      d.nodes
        .map((_, i) => i)
        .sort((a, b) => (b === selIdx) - (a === selIdx) || near.has(b) - near.has(a) || (deg[b] || 0) - (deg[a] || 0))
        .slice(0, room),
    );
    const nodes = d.nodes.map((nd, i) => ({
      id: String(i),
      label: nd.name,
      x: +pos[i].x.toFixed(2),
      y: +pos[i].y.toFixed(2),
      cls: i === selIdx ? "sel" : near.has(i) ? "near" : "",
      color: i === selIdx ? "var(--accent)" : "var(--blue)",
      rows: rows([["kind", nd.kind], ["file", `${tilde(nd.path)}:${nd.start_line}-${nd.end_line}`], ["edges drawn", String(deg[i] || 0)], ["store", gr.store], i === selIdx ? ["state", "selected"] : ["click", "select · drag to move"]]),
      onclick: () => {
        gr.sel = nd.name;
        gr.sym = null;
        paint();
        loadSymbol();
      },
    }));
    const edges = keep.filter((e) => shown.has(e.from) && shown.has(e.to)).map((e) => ({ a: String(e.from), b: String(e.to), ...edgeStyle(e, isDark()) }));
    liveCtl = drawLive(live, nodes.filter((_, i) => shown.has(i)), edges);
    fill(
      foot,
      el(
        "div",
        { class: "g-legend" },
        el("span", {}, el("i"), "extracted · resolved"),
        el("span", {}, el("i", { class: "dot" }), "inferred"),
        el("span", {}, el("i", { class: "dash" }), "ambiguous"),
      ),
      el("span", { class: "spacer" }),
      el("span", { class: "muted", text: `${n(Math.min(shown.size, d.shown || d.nodes.length))} of ${n(d.total)} symbols · ${n(edges.length)} edges · drag or click a node` }),
    );
  }

  function paintSide() {
    const s = gr.sym;
    const def = s && s.symbols && s.symbols[0];
    // Called by means calls: a reference or an alias is not a caller.
    const callers = ((s && s.callers) || []).filter((c) => !c.kind || c.kind === "calls");
    const callees = (s && s.callees) || [];
    const list = gr.dir === "in" ? callers : callees;
    const unresolvedList = (s && s.unresolved) || [];
    const comms = (gr.map && gr.map.store === gr.store && gr.map.communities) || [];
    fill(
      side,
      el(
        "div",
        { class: "g-side-scroll" },
        el(
          "div",
          { class: "g-sel" },
          el("span", { class: "eyebrow sm wide", text: "SELECTED" }),
          el("span", { class: "nm", text: gr.sel || "nothing yet" }),
          def ? pathSpan(`${tilde(def.path)}:${def.start_line}-${def.end_line}`, "t-mono-sm", `${def.path}:${def.start_line}-${def.end_line}`) : el("span", { class: "t-mono-sm", text: s && s.error ? s.error : s ? "no definition in this store" : "reading…" }),
          gr.defs && gr.defs.length > 1
            ? el(
                "div",
                { class: "g-defs" },
                el("span", { class: "eyebrow sm", text: `${gr.defs.length} definitions` }),
                gr.defs.map((d) =>
                  btn({ class: `g-def${d.name === gr.sel ? " on" : ""}`, onclick: () => ((gr.sel = d.name), (gr.sym = null), paint(), loadSymbol()) }, el("span", { class: "t-mono t-m", text: d.name }), pathSpan(`${tilde(d.path)}:${d.start_line}`, "t-mono-sm muted", `${d.path}:${d.start_line}`)),
                ),
              )
            : null,
          def ? el("div", { class: "row gap6" }, el("span", { class: "chipo blue", text: def.kind }), el("span", { class: "chipo", text: plural(def.end_line - def.start_line + 1, "line") }), el("span", { class: "chipo", text: gr.store }), (s.symbols || []).length > 1 ? el("span", { class: "chipo", text: `${s.symbols.length} definitions` }) : null) : null,
          el(
            "div",
            { class: "two" },
            btn({ class: "btn sm primary t125", disabled: gr.sel ? null : true, onclick: () => ((gr.br.sym = gr.sel), (gr.br.out = null), go("graph", "blast"), runReach()) }, "Blast radius"),
            btn({ class: "btn sm t125", disabled: gr.sel ? null : true, onclick: () => ((gr.pt.from = gr.sel), (gr.pt.to = ""), (gr.pt.out = null), go("graph", "path")) }, "Path from here"),
          ),
        ),
        tabs(
          [
            ["in", `Called by · ${callers.length}`],
            ["out", `Calls · ${callees.length}`],
          ],
          gr.dir,
          (v) => ((gr.dir = v), paintSide()),
          { cls: "small in-card" },
        ),
        el(
          "div",
          { class: "col gap4 pad8" },
          !s ? el("div", { class: "empty" }, "Reading…") : !list.length ? empty(gr.dir === "in" ? "Nothing in this store calls it." : "It calls nothing this store defines.") : null,
          list.map((c) =>
            btn(
              {
                class: "edge-row",
                onclick: () => {
                  if (c.confidence === "ambiguous") return toast(`${c.name} has ${c.definitions} definitions — open the file to tell which`);
                  gr.sel = c.name;
                  gr.data = null;
                  gr.sym = null;
                  loadExplore();
                },
              },
              el("span", { class: "col" }, el("span", { class: "n", text: c.name }), c.confidence === "ambiguous" ? el("span", { class: "w", text: `${c.definitions} definitions ▸` }) : pathSpan(`${tilde(c.path)}:${c.start_line}`, "w", `${c.path}:${c.start_line}`)),
              confBadge(c.confidence),
            ),
          ),
          gr.unres ? unresolvedList.map((u) => el("div", { class: "edge-row" }, el("span", { class: "col" }, el("span", { class: "n", text: u.name }), el("span", { class: "w", text: "no definition in any open store" })), confBadge("unresolved"))) : null,
          toggle(gr.unres, gr.unres ? "Hide unresolved" : "Show unresolved", (v) => ((gr.unres = v), loadSymbol()), { cls: "boxed sm" }),
        ),
        el(
          "div",
          { class: "subsys-wrap" },
          btn(
            { class: "group-row", "aria-expanded": String(gr.mapOpen), onclick: () => ((gr.mapOpen = !gr.mapOpen), gr.mapOpen && loadMap(), paintSide()) },
            el("span", { class: `caret${gr.mapOpen ? " open" : ""}` }, icon(I.chevRight, 13, { w: 2 })),
            el("span", { class: "t-b t-sm", text: "Subsystems" }),
            el("span", { class: "t-mono-sm", text: gr.map && gr.map.store === gr.store && !gr.map.error ? `${gr.map.shown || comms.length} of ${n(gr.map.total || comms.length)}` : "" }),
          ),
          gr.mapOpen
            ? el(
                "div",
                { class: "col gap6 pad-subsys" },
                !gr.map || gr.map.store !== gr.store ? el("div", { class: "empty" }, "Reading…") : gr.map.error ? errorBox(gr.map.error) : null,
                comms.map((m) =>
                  btn(
                    { class: "subsys", onclick: () => ((gr.sel = (m.hubs[0] || {}).name || m.label), (gr.data = null), (gr.sym = null), loadExplore()) },
                    el("span", { class: "row base nowrap" }, el("span", { class: "mono t-m t-sm grow", text: m.label }), el("span", { class: "t-mono-sm", text: plural(m.size, "symbol") })),
                    el("span", { class: "h", text: `hubs: ${(m.hubs || []).map((h) => h.name).join(" · ")}` }),
                  ),
                ),
                el("span", { class: "t-mono-sm pad2", text: "communities over settled calls and imports edges · no model" }),
              )
            : null,
        ),
      ),
    );
  }

  paintSide();
  if (gr.data && gr.data.storeName === gr.store) paint();
  setTimeout(() => {
    loadExplore();
    if (gr.mapOpen) loadMap();
  }, 0);
  return wrap;
}

async function runReach() {
  const b = gr.br;
  const name = b.sym.trim();
  if (!name) return;
  b.busy = true;
  b.out = null;
  if (current.view === VIEWS.graph) repaint();
  try {
    b.out = await api(`/api/impact?${new URLSearchParams({ name, store: gr.store, depth: String(b.hops), ...(b.verified ? {} : { all_edges: "1" }) })}`);
  } catch (e) {
    b.out = { error: e.message };
  }
  b.busy = false;
  if (current.view === VIEWS.graph) repaint();
}

function blastTab(picker) {
  const b = gr.br;
  const symInput = el("input", {
    class: "mono",
    value: b.sym,
    placeholder: "a symbol's exact name, e.g. main",
    "aria-label": "Symbol",
    "data-keep": "br-sym",
    oninput: (e) => (b.sym = e.target.value),
    onkeydown: (e) => e.key === "Enter" && runReach(),
  });
  const imp = b.out && b.out.impact;
  const parts = [
    el(
      "div",
      { class: "ctrl-card" },
      el("span", { class: "eyebrow", text: "IF THIS CHANGES" }),
      el("div", { class: "box focus h34 grow-box" }, symInput),
      el("div", { class: "dial" }, el("span", { class: "eyebrow", text: "HOPS" }), String(b.hops), btn({ class: "pm", "aria-label": "Fewer hops", onclick: () => ((b.hops = Math.max(1, b.hops - 1)), repaint()) }, "−"), btn({ class: "pm", "aria-label": "More hops", onclick: () => ((b.hops = Math.min(6, b.hops + 1)), repaint()) }, "+")),
      toggle(b.verified, "Verified edges only", (v) => ((b.verified = v), repaint(), b.out && runReach()), { cls: "boxed h34" }),
      picker,
      btn({ class: "btn md primary", onclick: runReach }, "Reach"),
    ),
  ];
  if (b.busy) parts.push(el("div", { class: "row gap10 muted" }, el("span", { class: "spinner" }), "Walking the edges backwards…"));
  else if (b.out && b.out.error) parts.push(el("div", { class: "card" }, errorBox(b.out.error)));
  else if (!imp) {
    let hubs = (gr.map && gr.map.store === gr.store ? gr.map.communities || [] : []).flatMap((c) => c.hubs || []).slice(0, 3);
    // A store too small for communities still has symbols: the busiest ones
    // in its overview graph are where to start.
    if (!hubs.length && data.graphpeek && graphPeekFor === gr.store) {
      const g = data.graphpeek;
      const deg = {};
      for (const e of g.edges || []) {
        deg[e.from] = (deg[e.from] || 0) + 1;
        deg[e.to] = (deg[e.to] || 0) + 1;
      }
      hubs = (g.nodes || [])
        .map((nd, i) => ({ name: nd.name, path: nd.path, kind: nd.kind, d: deg[i] || 0 }))
        .filter((h) => h.name && h.kind !== "module" && h.kind !== "file" && !/[./]/.test(h.name))
        .sort((a, b) => b.d - a.d)
        .slice(0, 3);
    }
    // Opened straight on this tab, the store's hubs are not read yet: read
    // them, so the starting points are symbols this store really defines.
    if (gr.store && !(gr.map && gr.map.store === gr.store)) {
      const want = gr.store;
      api(`/api/map?${new URLSearchParams({ store: want, shown: "12" })}`)
        .then((out) => {
          gr.map = { ...out, store: want };
          if (state.route.page === "graph" && state.route.parts[0] === "blast") repaint();
        })
        .catch((e) => (gr.map = { error: e.message, store: want, communities: [] }));
    }
    parts.push(
      el(
        "div",
        { class: "auto-fit m240" },
        (hubs.length ? hubs : gr.sel ? [{ name: gr.sel }] : []).map((h) => btn({ class: "kind-card", onclick: () => ((b.sym = h.name), runReach()) }, el("span", { class: "mono t-m", text: h.name }), h.path ? el("span", { class: "d col min0" }, pathSpan(store(gr.store) ? relTo(store(gr.store), h.path) : tilde(h.path), "t-mono-sm", h.path), "A hub here — what depends on it?") : el("span", { class: "d", text: "What depends on it?" }))),
      ),
    );
  } else parts.push(blastResult(imp, b.out.headline));
  return el("div", { class: "gr-pad" }, parts);
}

function blastResult(imp, headline) {
  const b = gr.br;
  const reached = imp.reached || [];
  const files = imp.files || [];
  const inferred = (imp.inferred || 0) + (imp.ambiguous || 0);
  const hopsMax = Math.max(0, ...reached.map((r) => r.hop));
  const q = b.q.toLowerCase();
  const filtered = reached.filter((r) => (b.hop === "all" || (b.hop === "3" ? r.hop >= 3 : String(r.hop) === b.hop)) && (b.edge === "all" || (b.edge === "inferred" ? r.confidence === "inferred" || r.confidence === "ambiguous" : r.confidence === "resolved" || r.confidence === "extracted")) && (!q || `${r.name} ${r.path}`.toLowerCase().includes(q)));
  const g = grid({
    key: "blast",
    caption: "What depends on the symbol",
    rows: filtered,
    sort: "hop",
    per: 10,
    empty: "Nothing reached matches.",
    onClear: () => ((b.q = ""), (b.hop = "all"), (b.edge = "all"), repaint()),
    columns: [
      { key: "name", label: "Reached", cls: "mm cap180", sort: (r) => r.name, render: (r) => r.name },
      // Where it reaches the symbol: the call site (at), not where it is defined.
      { key: "where", label: "Where", cls: "ms cap180", sort: (r) => `${r.path}:${String(r.at ?? r.line).padStart(8, "0")}`, render: (r) => pathSpan(`${tilde(r.path)}:${r.at ?? r.line}`, "", `${r.path}:${r.at ?? r.line} · defined at line ${r.line}`) },
      { key: "edge", label: "Edge", sort: (r) => r.confidence, render: (r) => el("span", { class: `badge ${r.confidence === "inferred" || r.confidence === "ambiguous" ? "amber" : "blue"}`, text: `${r.kind} · ${r.confidence}` }) },
      { key: "hop", label: "Hop", cls: "ms r", sort: (r) => r.hop, render: (r) => String(r.hop) },
    ],
  });
  const qInput = el("input", { value: b.q, placeholder: "Filter by name or file", "data-keep": "br-q", "aria-label": "Filter reached", oninput: (e) => ((b.q = e.target.value), repaint()) });
  // The reverse mini-graph: the symbol in the middle, the nearest callers
  // around it, each joined to whatever it reached the symbol through.
  const drawn = [{ name: imp.name, hop: 0 }, ...reached.filter((r) => r.hop <= 2).slice(0, 21)];
  const ring = drawn.slice(1);
  const mini = el("div", { class: "g-live mini" });
  const nodes = drawn.map((r, i) => {
    const a = (i / Math.max(1, ring.length)) * Math.PI * 2 - Math.PI / 2;
    const rad = r.hop === 1 ? 0.55 : 0.85;
    const x = i === 0 ? 50 : Math.max(10, Math.min(90, 50 + Math.cos(a) * rad * 42));
    const y = i === 0 ? 52 : Math.max(10, Math.min(92, 52 + Math.sin(a) * rad * 40));
    return { id: r.name + (i ? `#${i}` : ""), label: r.name, x: +x.toFixed(2), y: +y.toFixed(2), cls: i === 0 ? "sel" : r.hop > 1 ? "dim" : "near", color: i === 0 ? "var(--accent)" : "var(--blue)", rows: i === 0 ? rows([["role", "the symbol that changes"], ["reaches", plural(reached.length, "symbol")]]) : rows([["hop", String(r.hop)], ["file", `${tilde(r.path)}:${r.at ?? r.line}`], ["edge", `${r.kind} · ${r.confidence}`]]) };
  });
  const idOf = (name) => (nodes.find((nd) => nd.label === name) || {}).id;
  const edges = nodes.slice(1).map((nd, i) => {
    const r = ring[i];
    const to = r.hop === 1 ? nodes[0].id : idOf(r.via) || nodes[0].id;
    return { a: nd.id, b: to, ...edgeStyle(r, isDark()) };
  });
  setTimeout(() => drawLive(mini, nodes, edges), 0);
  const top = files[0] ? files[0].symbols : 1;
  return el(
    "div",
    { class: "split s-1-1" },
    el(
      "div",
      { class: "stack" },
      el(
        "div",
        { class: "card pad" },
        el("div", { class: "t-m t-sm pretty big14", text: headline || `${plural(reached.length, "definition")} in ${plural(files.length, "file")} reach ${imp.name} within ${plural(b.hops, "hop")}` }),
        imp.hidden ? el("div", { class: "muted t-xs", text: `${n(imp.hidden)} more beyond the ${n(reached.length)} listed — narrow the hops to see them all.` }) : null,
        el("div", { class: "q3 tb-line" }, [["REACHED", n(reached.length + (imp.hidden || 0))], ["FILES", n(files.length)], ["INFERRED", n(inferred)]].map(([k, v]) => el("div", { class: "stat-inline" }, el("span", { class: "eyebrow sm", text: k }), el("span", { class: "v", text: v })))),
      ),
      el(
        "div",
        { class: "card" },
        el(
          "div",
          { class: "filterbar" },
          el("div", { class: "box h28 w180 full-sm" }, icon(I.searchSm, 13, { w: 1.8 }), qInput),
          seg(
            [
              ["all", "All hops"],
              ["1", "1"],
              ["2", "2"],
              ["3", "3+"],
            ],
            b.hop,
            (v) => ((b.hop = v), repaint()),
          ),
          dropdown({
            label: "Edge",
            value: b.edge,
            options: [
              ["all", "All edges"],
              ["resolved", "Resolved"],
              ["inferred", "Inferred"],
            ],
            onChange: (v) => ((b.edge = v), repaint()),
          }),
        ),
        g.node,
      ),
    ),
    el(
      "div",
      { class: "stack" },
      el("div", { class: "mini-graph" }, el("span", { class: "label", text: reached.length > ring.length ? `${n(reached.length - ring.length)} beyond the ${ring.length} drawn` : `reverse reachability · ${plural(hopsMax, "hop")}` }), mini),
      el(
        "div",
        { class: "card" },
        el("div", { class: "card-h tight" }, el("span", { class: "card-t sm grow", text: "Files to look at" }), lnk("Copy list", () => copy(files.map((f) => f.path).join("\n"), `Copied ${plural(files.length, "path")}`))),
        files.slice(0, 12).map((f) => {
          const b2 = bar((f.symbols / top) * 100, "h5 accent");
          return el("div", { class: "file-bar-row", "data-tip": f.path, "data-tip-rows": rows([["definitions reached", String(f.symbols)], ["nearest hop", String(f.nearest)]]) }, pathSpan(tilde(f.path), "", f.path), b2, el("span", { class: "right muted", text: String(f.symbols) }));
        }),
      ),
    ),
  );
}

async function runPath(showInferred) {
  const p = gr.pt;
  if (!p.from.trim() || !p.to.trim()) return;
  p.busy = true;
  p.showInferred = !!showInferred;
  if (current.view === VIEWS.graph) repaint();
  const all = showInferred || (!p.verified && !p.strict);
  try {
    p.out = await api(`/api/trace?${new URLSearchParams({ from: p.from.trim(), to: p.to.trim(), store: gr.store, ...(all ? { all_edges: "1" } : {}) })}`);
  } catch (e) {
    p.out = { error: e.message };
  }
  p.busy = false;
  if (current.view === VIEWS.graph) repaint();
}

// What a symbol calls, for the Path tab when only its start is known: "Path
// from here" opens with the places the walk can go, one press from a trace.
async function loadLeads() {
  const p = gr.pt;
  const name = p.from.trim();
  const key = `${gr.store}|${name}`;
  if (!name || p.leadFor === key) return;
  p.leadFor = key;
  p.leads = null;
  try {
    const sym = await api(`/api/symbol?${new URLSearchParams({ name, store: gr.store, k: "40" })}`);
    if (p.leadFor !== key) return;
    p.leads = { callees: sym.callees || [], error: sym.error || null };
  } catch (e) {
    if (p.leadFor === key) p.leads = { callees: [], error: e.message };
  }
  if (current.view === VIEWS.graph) repaint();
}

function pathTab(picker) {
  const p = gr.pt;
  const from = el("input", { class: "inp mono h34", value: p.from, spellcheck: "false", "aria-label": "From", "data-keep": "pt-from", placeholder: "from", oninput: (e) => (p.from = e.target.value), onkeydown: (e) => e.key === "Enter" && runPath() });
  const to = el("input", { class: "inp mono h34", value: p.to, spellcheck: "false", "aria-label": "To", "data-keep": "pt-to", placeholder: "to", oninput: (e) => (p.to = e.target.value), onkeydown: (e) => e.key === "Enter" && runPath() });
  const parts = [
    el(
      "div",
      { class: "ctrl-card" },
      el("span", { class: "eyebrow", text: "FROM" }),
      from,
      btn({ class: "btn icon", "aria-label": "Swap", "data-tip": "Swap", onclick: () => (([p.from, p.to] = [p.to, p.from]), repaint()) }, icon(I.swap, 14, { w: 1.8 })),
      el("span", { class: "eyebrow", text: "TO" }),
      to,
      toggle(p.verified, "Prefer verified", (v) => ((p.verified = v), repaint()), { tip: "List extracted and resolved chains first" }),
      toggle(p.strict, "Strict", (v) => ((p.strict = v), repaint()), { tip: "Refuse to cross a name with several definitions" }),
      picker,
      btn({ class: "btn md primary", onclick: () => runPath() }, "Find the path"),
    ),
  ];
  const t = p.out && p.out.trace;
  if (!p.busy && !p.out && p.from.trim() && !p.to.trim()) {
    loadLeads();
    const L = p.leads;
    parts.push(
      el(
        "div",
        { class: "card" },
        el("div", { class: "card-h" }, el("span", { class: "card-t grow" }, "Where ", el("span", { class: "mono", text: p.from.trim() }), " leads"), meta("pick a destination, or type one in To")),
        !L
          ? el("div", { class: "row gap10 muted pad" }, el("span", { class: "spinner" }), "Reading what it calls…")
          : L.error
            ? errorBox(L.error)
            : !L.callees.length
              ? empty(`${p.from.trim()} calls nothing this store defines. Type a destination in To to look for a path the other way round, or swap.`)
              : el(
                  "div",
                  { class: "col gap4 pad8" },
                  L.callees.map((c) =>
                    btn(
                      { class: "edge-row", onclick: () => ((p.to = c.name), runPath()) },
                      el("span", { class: "col" }, el("span", { class: "n", text: c.name }), c.path ? pathSpan(`${tilde(c.path)}:${c.start_line}`, "w", `${c.path}:${c.start_line}`) : null),
                      confBadge(c.confidence),
                    ),
                  ),
                ),
      ),
    );
  }
  if (p.busy) parts.push(el("div", { class: "row gap10 muted" }, el("span", { class: "spinner" }), "Walking the edges…"));
  else if (p.out && p.out.error) parts.push(el("div", { class: "card" }, errorBox(p.out.error)));
  else if (t) {
    const chain = t.chain;
    const steps = (chain && chain.steps) || [];
    const sum = chain ? chain.summary : null;
    const seamAfter = (i) => steps[i + 1] && (steps[i].to_path !== steps[i + 1].from_path || steps[i].to_line !== steps[i + 1].from_line);
    const nodesList = steps.length ? [...steps.map((st, i) => ({ name: st.from, where: `${tilde(st.from_path)}:${st.from_line}`, edge: st, seam: i > 0 && seamAfter(i - 1) })), { name: steps[steps.length - 1].to, where: `${tilde(steps[steps.length - 1].to_path)}:${steps[steps.length - 1].to_line}`, edge: null }] : [];
    parts.push(
      el(
        "div",
        { class: "split s-1-1" },
        el(
          "div",
          { class: "card" },
          sum && sum.hypothesis ? el("div", { class: "card-note ink2" }, el("b", { text: "A hypothesis, not a finding. " }), "A hop here was matched by name or crosses a seam; read the seams before you rely on it.") : null,
          el(
            "div",
            { class: "card-b line-row" },
            el("span", { class: "eyebrow sm wide", text: "ANSWER" }),
            el("span", { class: "t-m pretty big14", text: t.answer }),
            !steps.length && !p.strict && !p.showInferred && !t.all_edges ? lnk("Show the inferred chain", () => runPath(true)) : null,
          ),
          steps.length
            ? el(
                "div",
                { class: "chain" },
                nodesList.map((nd, i) =>
                  el(
                    "div",
                    { class: "col" },
                    el("div", { class: "node" }, el("span", { class: `cd${nd.edge ? "" : " end"}` }), el("span", { class: "n", text: nd.name }), pathSpan(nd.where, "w")),
                    nd.edge
                      ? el(
                          "div",
                          { class: `edge ${nd.edge.confidence}` },
                          el("span", { class: "t-mono-sm", text: nd.edge.kind }),
                          confBadge(nd.edge.confidence),
                          nd.edge.definitions > 1 ? el("span", { class: "t-mono-sm amber-ink", text: `${nd.edge.to}: ${nd.edge.definitions} definitions` }) : null,
                          seamAfter(i) ? el("span", { class: "t-mono-sm amber-ink", text: "seam · the next hop leaves from another definition" }) : null,
                        )
                      : null,
                  ),
                ),
              )
            : null,
          sum ? el("div", { class: "card-foot", text: `${plural(sum.hops, "hop")} · ${sum.extracted} extracted · ${sum.resolved} resolved · ${sum.inferred} inferred · ${sum.ambiguous} ambiguous${sum.seams ? ` · ${plural(sum.seams, "seam")}` : ""}` }) : null,
        ),
        el(
          "div",
          { class: "card" },
          el("div", { class: "card-h" }, el("span", { class: "card-t grow", text: "Supporting lines" }), btn({ class: "btn sm dark t125", disabled: (t.lines || []).length ? null : true, onclick: () => copy(p.out.evidence || (t.lines || []).map((l) => `${l.at}   ${l.code.trim()}`).join("\n"), "Evidence copied as plain text") }, "Copy as evidence")),
          !(t.lines || []).length ? empty("No chain, so no lines to quote.") : null,
          (t.lines || []).map((l) =>
            el(
              "div",
              { class: "support" },
              el("div", { class: "row nowrap" }, pathSpan(tilde(l.at), "mono t-m t-xs grow", l.at), pill(l.mark, /supporting/.test(l.mark) ? "blue" : "grey", { dot: false })),
              el("div", { class: "code sm", text: l.code }),
              el("span", { class: "t-mono-sm", text: l.hop }),
            ),
          ),
        ),
      ),
    );
  }
  return el("div", { class: "gr-pad" }, parts);
}

// -------------------------------------------------------------------- agents

const ag = { client: "", method: "json", reveal: null };

VIEWS.agents = {
  needs: () => ["agents", "ledger"],
  live: ["agents", "ledger"],
  render(route) {
    const tab = route.parts[0] || "connected";
    const a = data.agents || {};
    const conns = connectedRows(a);
    const clients = clientRows();
    const found = clients.filter((c) => c.found);
    const unreg = found.filter((c) => !c.registered);
    const open = a.endpoint ? a.endpoint.open !== false : true;
    const faults = (a.doctor || []).filter((c) => c.fault).length;
    const body = { connected: agConnected, add: agAdd, tools: agTools, health: agHealth }[tab] || agConnected;
    return el(
      "div",
      { class: "page" },
      el(
        "div",
        { class: "head" },
        el("div", { class: "titles" }, el("div", { class: "row nowrap gap10" }, el("div", { class: "h1", text: "Agents" }), pill(open ? "answering" : "stopped", open ? "green" : "grey", { pulse: open })), el("div", { class: "lead", text: "One endpoint on this machine for every client. No per-client process, no second copy of the index." })),
        el("div", { class: "endpoint-box" }, el("span", { class: "eyebrow sm", text: "MCP" }), el("span", { class: "u", text: a.endpoint?.url || "" }), btn({ class: "btn xs soft", onclick: () => copy(a.endpoint?.url || "") }, "Copy")),
        toggle(open, open ? "Endpoint on" : "Endpoint off", async (v) => {
          // Closing it cuts every connected agent off, so it asks first.
          if (!v) {
            const live = connectedRows(a).filter((c) => !c.waiting).length;
            const ok = await ask({ title: "Stop answering agents?", body: `${live ? `${plural(live, "connected client")} will get a clear refusal on ${live === 1 ? "its" : "their"} next call` : "Every agent will get a clear refusal"} until you turn the endpoint back on. The stores, the watchers and this page keep running.`, ok: "Stop the endpoint", danger: true });
            if (!ok) return;
          }
          const out = await act(() => post("/api/endpoint", { open: v }), v ? "Endpoint answering again" : "Endpoint closed — agents get a clear refusal");
          if (out) await load("agents", true), repaint();
        }, { cls: "boxed h34 strong" }),
      ),
      tabs(
        [
          ["connected", "Connected", conns.length],
          ["add", "Add a client", unreg.length],
          ["tools", "Tools", (a.tools || []).length],
          ["health", "Health", faults],
        ],
        tab,
        (t) => go("agents", t === "connected" ? undefined : t),
      ),
      body(a, clients),
    );
  },
};

// Live connections first, then every registered client that has not called
// yet, so a client registered a minute ago is on the list, waiting.
function connectedRows(a) {
  const conns = a.connections || [];
  const waiting = registeredClients()
    .filter((c) => !conns.some((x) => sameClient(x.name, c.name)))
    .map((c) => ({ name: c.name, waiting: true }));
  return [...conns, ...waiting];
}

function agConnected(a) {
  const conns = connectedRows(a);
  const rows = data.ledger?.rows || [];
  const lastOf = (name) => (rows.find((r) => sameClient(r.client, name)) || {}).at;
  return el(
    "div",
    { class: "split s-17-1" },
    el(
      "div",
      { class: "card" },
      !conns.length
        ? el(
            "div",
            { class: "card-b" },
            el("div", { class: "row gap12" }, el("div", { class: "col gap2 grow" }, el("span", { class: "t-b", text: "No client is talking to semlith yet" }), el("span", { class: "muted t-sm", text: "Register one and restart it — it shows up here on its first call." })), btn({ class: "btn primary", onclick: () => go("agents", "add") }, "Add a client")),
          )
        : el(
            "div",
            { class: "tw" },
            el(
              "table",
              { "aria-label": "Connected clients" },
              el("thead", {}, el("tr", {}, ["Client", "State", "Version", "Transport", "Queries", "Last query"].map((h, i) => el("th", { class: i >= 4 ? "r" : null, text: h })))),
              el(
                "tbody",
                {},
                conns.map((c) => {
                  if (c.waiting)
                    return el(
                      "tr",
                      {},
                      el("td", { class: "t-m", text: c.name }),
                      el("td", {}, pill("registered", "blue", { dot: false, tip: "Restart it — it shows here as active on its first call" })),
                      el("td", { class: "ms", text: "—" }),
                      el("td", { class: "ms", text: "—" }),
                      el("td", { class: "m r", text: "0" }),
                      el("td", { class: "ms r", text: "waiting for its first call" }),
                    );
                  const active = c.seen && Date.now() / 1000 - c.seen < 600;
                  const last = lastOf(c.name) || c.seen;
                  return el(
                    "tr",
                    {},
                    el("td", { class: "t-m" }, c.name, c.sessions > 1 ? el("span", { class: "muted t-xs", text: ` · ${c.sessions} sessions` }) : null),
                    el("td", {}, pill(active ? "active" : "idle", active ? "green" : "grey", { pulse: active })),
                    el("td", { class: "ms", text: c.version || "—" }),
                    el("td", { class: "ms", text: `${c.transport}${a.forwarding && c.transport === "stdio" ? " · proxy" : ""}` }),
                    el("td", { class: "m r", text: n(c.queries) }),
                    el("td", { class: "ms r", text: last ? ago(last) : "never" }),
                  );
                }),
              ),
            ),
          ),
    ),
    el(
      "div",
      { class: "stack" },
      el("div", { class: "card pad" }, el("span", { class: "eyebrow", text: "What the tool list costs" }), el("span", { class: "mono t-b big18", text: `${n(a.tool_list_tokens)} tokens` }), el("span", { class: "muted t-xs", text: `per session, read once before the agent asks anything · ${plural((a.tools || []).filter((t) => t.listed !== false).length, "tool")} listed · ${n(a.tool_list_bytes)} bytes · counted ${a.tool_list_tier === "tokenizer" ? "by the model's tokenizer" : `as ${a.tool_list_tier}`}` })),
      el("div", { class: "card pad" }, el("span", { class: "card-t", text: "After you register a client" }), el("span", { class: "muted t-sm pretty", text: "Restart it. Ask it something about your code — “use semlith to find where X happens”. The call lands in the Ledger with what it was sent." }), lnk("Open the ledger →", () => go("ledger"))),
    ),
  );
}

function agAdd(a, clients) {
  const found = clients.filter((c) => c.found);
  const reg = clients.filter((c) => c.registered);
  const unreg = found.filter((c) => !c.registered);
  const sel = clients.find((c) => c.name === ag.client) || unreg[0] || found[0] || clients[0];
  if (!sel) return el("div", { class: "card" }, empty("No client is documented in this build."));
  ag.client = sel.name;
  const groups = [
    ["terminal", "TERMINAL"],
    ["editor", "EDITORS"],
    ["desktop", "DESKTOP"],
  ];
  const register = async (names, action) => {
    const out = await act(() => post("/api/agents/register", { clients: names, action, confirm: true }), action === "unregister" ? `Removed semlith from ${names.join(", ")}` : names.length === 1 ? `Registered ${names[0]} — restart it to pick semlith up` : `Registered ${names.length} clients · each config backed up first`);
    if (out) {
      if ((out.failed || []).length) toast(out.failed.map((f) => `${f.client}: ${f.error}`).join(" · "), true);
      await load("agents", true);
      repaint();
    }
  };
  const registerCmd = (sel.client.stanzas || []).find((s) => s.register);
  const file = (sel.client.stanzas || []).find((s) => s.path);
  const stanzaJson = (sel.client.stanzas || []).find((s) => s.format === "json" && !s.register);
  const cmds = (sel.client.stanzas || []).filter((s) => s.format === "sh" && !s.unregister).map((s) => s.text.replace(/\s+/g, " "));
  return el(
    "div",
    { class: "split s-1-12" },
    el(
      "div",
      { class: "card" },
      el("div", { class: "card-h" }, el("span", { class: "card-t grow", text: "Clients" }), meta(`${found.length} found · ${reg.length} registered`)),
      groups.map(([g, label]) => {
        const list = clients.filter((c) => c.group === g || (g === "editor" && c.group === "editors"));
        if (!list.length) return null;
        return el(
          "div",
          { class: "col" },
          el("div", { class: "group-label", text: label }),
          list.map((c) =>
            btn(
              { class: `client-row${c.name === sel.name ? " on" : ""}`, "aria-current": c.name === sel.name ? "true" : null, onclick: () => ((ag.client = c.name), repaint()) },
              dot(c.registered ? "green" : c.found ? "amber" : ""),
              el("span", { class: "grow t-m t-sm", text: c.name }),
              c.registered ? pill("registered", "green", { dot: false }) : el("span", { class: "t-mono-sm", text: c.found ? "found" : "not found" }),
            ),
          ),
        );
      }),
      el("div", { class: "card-foot sans" }, el("span", { class: "muted grow", text: unreg.length ? `${plural(unreg.length, "client")} found but not registered` : "Every client found here is registered" }), btn({ class: "btn sm dark", disabled: unreg.length ? null : true, onclick: () => register(unreg.map((c) => c.name), "register") }, `Register ${unreg.length}`)),
    ),
    el(
      "div",
      { class: "card" },
      el(
        "div",
        { class: "card-b line-row" },
        el("div", { class: "row nowrap gap10" }, el("div", { class: "col gap2 grow" }, el("span", { class: "t-b big15", text: sel.name }), el("span", { class: "muted t-sm", text: sel.found ? `${sel.group} · ${registerCmd ? "has its own registration command" : "semlith writes its config file"}` : `${sel.group} · not found on this machine` })), pill(sel.registered ? "registered" : sel.found ? "not registered" : "not found", sel.registered ? "green" : sel.found ? "amber" : "grey", { dot: false })),
      ),
      el(
        "div",
        { class: "card-b" },
        sel.found
          ? el(
              "div",
              { class: "one-click" },
              el("div", { class: "col gap2 grow" }, el("span", { class: "t-m t-sm", text: sel.registered ? `Registered${sel.report.scope ? ` at ${sel.report.scope} scope` : ""} — every project sees it` : registerCmd ? "Semlith runs the client's own command for you" : "Semlith writes the file, after backing it up beside itself" }), el("span", { class: "t-mono-sm anywhere", text: shortPaths(sel.write, 40), "data-tip": sel.write })),
              btn({ class: `btn ${sel.registered ? "" : "primary"}`, onclick: () => register([sel.name], sel.registered ? "unregister" : "register") }, sel.registered ? "Unregister" : "Register"),
            )
          : el("div", { class: "notice plain", text: "Not installed on this machine. Install it and come back — or paste the config below wherever it lives." }),
        sel.client.note ? clientNote(sel.client.note) : null,
        tabs(
          [
            ["json", "Config file"],
            ["cli", "Terminal"],
          ],
          ag.method,
          (m) => ((ag.method = m), repaint()),
          { cls: "small" },
        ),
        ag.method === "json"
          ? [codeBlock(stanzaJson ? stanzaJson.text : mcpJson(), { word: "Config copied" }), el("span", { class: "muted t-xs", text: `Paste into ${file ? file.path : "the client's MCP settings"}.${/SEMLITH_AGENT_KEY|Bearer/.test(stanzaJson ? stanzaJson.text : mcpJson()) ? ` Export ${a.key_env || "SEMLITH_AGENT_KEY"} in the shell that launches it.` : ""}` })]
          : [
              cmds.length ? cmds.map((c) => copyField(c, { cls: "auto", btn: "xs" })) : el("span", { class: "muted t-sm", text: "This client has no terminal command; it reads a file." }),
              el("span", { class: "muted t-xs", text: `The HTTP form needs ${a.key_env || "SEMLITH_AGENT_KEY"} exported; the subprocess form reads the key itself.` }),
            ],
      ),
    ),
  );
}

function agTools(a) {
  const all = a.tools || [];
  // Listed first: those are what an agent is offered. The rest answer from
  // the command line and this portal.
  const tools = [...all.filter((t) => t.listed !== false), ...all.filter((t) => t.listed === false)];
  const listed = all.filter((t) => t.listed !== false).length;
  return el(
    "div",
    { class: "card" },
    el("div", { class: "card-h" }, el("span", { class: "card-t grow", text: "What an agent can call" }), meta(`${listed} of ${plural(all.length, "tool")} offered to agents · ${n(a.tool_list_bytes)} bytes · ${n(a.tool_list_tokens)} tokens per session, counted ${a.tool_list_tier === "tokenizer" ? "by the model" : `as ${a.tool_list_tier || "an estimate"}`}`)),
    el(
      "div",
      { class: "tw" },
      el(
        "table",
        { "aria-label": "MCP tools" },
        el("thead", {}, el("tr", {}, el("th", { text: "Tool" }), el("th", { text: "What it answers" }), el("th", { class: "r", text: "Typical answer" }))),
        el(
          "tbody",
          {},
          tools.map((t) =>
            el(
              "tr",
              {},
              el("td", { class: "mm nowrap" }, t.name, t.listed === false ? " " : null, t.listed === false ? el("span", { class: "badge outline", "data-tip": "Not in the tool list agents are offered: run it from the command line or this portal, or set SEMLITH_MCP_TOOLS=all for the server", text: "CLI and portal" }) : null),
              el("td", { class: "ink2 t13", text: t.answers || t.about }),
              el("td", { class: "ms r nowrap", "data-tip": t.typical_source === "ledger" ? "Median of this machine's own answers" : t.typical_source === "estimate" ? "An estimate until this machine has five answers from it" : null }, t.typical_tokens ? `${t.typical_source === "ledger" ? "" : "~"}${n(t.typical_tokens)} tok` : "—"),
            ),
          ),
        ),
      ),
    ),
  );
}

const AGENTFILE_TONE = { current: "green", linked: "green", present: "green", installed: "green", stale: "amber", missing: "amber", absent: "grey", paste: "grey" };

function agHealth(a) {
  const rows = a.doctor || [];
  const reach = rows.filter((c) => c.registered && !c.fault).length;
  const fix = rows.filter((c) => c.fault).length;
  const missing = rows.filter((c) => !c.in_use && !c.present).length;
  const recheck = async () => {
    await act(() => load("agents", true), `Checked ${plural(rows.length, "client")}`);
    repaint();
  };
  return el(
    "div",
    { class: "stack" },
    el(
      "div",
      { class: "card" },
      el("div", { class: "card-h" }, el("span", { class: "card-t grow", text: "Can each client reach semlith?" }), meta(`${reach} can reach it · ${fix} to fix · ${missing} not installed`), btn({ class: "btn xs", onclick: recheck }, "Check again")),
      el(
        "div",
        { class: "tw" },
        el(
          "table",
          { "aria-label": "Can each client reach semlith" },
          el("thead", {}, el("tr", {}, el("th", { text: "Client" }), el("th", { text: "Semlith" }), el("th", { text: "Skill, hook and rule" }), el("th", { text: "To fix" }))),
          el(
            "tbody",
            {},
            rows.map((c) => {
              const tags = [
                ["skill", c.skill],
                [`hook${c.hook_mode ? ` (${c.hook_mode})` : ""}`, c.hook],
                ["rule", c.rules],
              ].filter(([, st]) => st && st !== "paste");
              return el(
                "tr",
                {},
                el("td", { class: "t-m nowrap", text: c.name }),
                el("td", {}, pill(c.registered ? `registered (${c.scope || "user"})` : c.in_use || c.present ? "found · not registered" : "not installed", c.fault ? "red" : c.registered ? "green" : c.in_use || c.present ? "amber" : "grey", { dot: false })),
                el("td", {}, tags.length ? el("div", { class: "row gap4" }, tags.map(([k, st]) => pill(`${k} ${st}`, AGENTFILE_TONE[st] || "grey", { dot: false, sm: true }))) : el("span", { class: "muted", text: "—" })),
                el("td", { class: "ms", text: c.explain || c.repair || ((c.disabled_in || []).length ? `switched off in ${c.disabled_in.length} folder${c.disabled_in.length === 1 ? "" : "s"}` : c.in_use && !c.registered ? "Register it on Add a client" : "—") }),
              );
            }),
          ),
        ),
      ),
    ),
    el(
      "div",
      { class: "auto-fit m220" },
      (data.privacy?.rules || [])
        .filter((r) => r.applicable !== false && r.check)
        .slice(0, 6)
        .map((r) => el("div", { class: "check-card" }, el("div", { class: "row nowrap" }, dot(r.ok ? "green" : "amber"), el("span", { class: "eyebrow grow min0 ell", text: r.id, title: r.id }), el("span", { class: `t-mono t-m ${r.ok ? "green-ink" : "amber-ink"}`, text: r.ok ? "holds" : "to fix" })), el("span", { class: "ink2 t-sm anywhere", text: r.check }), r.manual && !r.ok ? copyField(r.manual) : null)),
    ),
  );
}

// -------------------------------------------------------------------- ledger

const lg = { tab: "sessions", filter: null, q: "", store: "all", tier: "all", zero: false, replay: null };

VIEWS.ledger = {
  needs: (route) => ["ledger", "stores", "cloud"].concat((route.parts[0] || "sessions") === "replay" ? ["replay"] : []),
  live: ["ledger"],
  render(route) {
    lg.tab = route.parts[0] || "sessions";
    const L = data.ledger || {};
    const rec = recordingWord();
    const recState = L.recording && typeof L.recording === "object" ? L.recording : { on: rec === "on", reason: rec === "on" ? null : "flag" };
    const lockedOff = recState.reason === "flag" || recState.reason === "env";
    const byClient = Object.entries(L.by_client || {}).sort((a, b) => b[1] - a[1]);
    const maxClient = byClient.length ? byClient[0][1] : 1;
    const sessionsAll = L.sessions || [];
    const rowsAll = L.rows || [];
    const match = (c) => !lg.filter || sameClient(c, lg.filter);
    const zeroShare = L.queries ? (L.zero_hit / L.queries) * 100 : 0;
    const lastRow = rowsAll[0];
    const parts = [
      el(
        "div",
        { class: "head" },
        el("div", { class: "titles" }, el("div", { class: "row nowrap gap10" }, el("div", { class: "h1", text: "Retrieval ledger" }), pill(rec === "on" ? "recording" : rec === "paused" ? "paused" : "off", rec === "on" ? "green" : "grey", { pulse: rec === "on" })), el("div", { class: "lead", text: data.cloud?.signed_in ? "Every query an agent ran and what it was sent — recorded on this machine, hash-chained; the stores you sync send their rows, never the text." : "Every query an agent ran and what it was sent — recorded on this machine, hash-chained, never uploaded." }), syncLine()),
        toggle(
          rec === "on",
          rec === "on" ? "Recording" : lockedOff ? `Off — ${recState.reason === "env" ? "SEMLITH_LEDGER=0" : "--no-ledger"}` : "Recording paused",
          async (v) => {
            const out = await act(() => post("/api/ledger/recording", { on: v }), v ? "Recording again" : "Paused — nothing is recorded until you resume");
            if (out) await loadMany(["ledger", "about"], true), paintChrome(), repaint();
          },
          { cls: "boxed strong", disabled: lockedOff, tip: lockedOff ? "This daemon was started with recording off; restart it without the flag to turn it on here" : null },
        ),
        btn({ class: "btn dark", onclick: () => go("reports") }, "Build a report"),
      ),
    ];
    if (!L.queries)
      parts.push(
        el(
          "div",
          { class: "drop-hint sm" },
          el("div", { class: "col gap2 grow" }, el("span", { class: "t-b", text: "Nothing recorded yet" }), el("span", { class: "muted t-sm", text: rec === "on" ? "The ledger fills in the moment an agent asks something." : "Recording is off, so nothing will be listed until it is on." })),
          btn({ class: "btn", onclick: () => go("agents") }, "Check agents"),
        ),
      );
    if (L.intact === false)
      parts.push(
        el(
          "div",
          { class: "notice red" },
          icon(I.alert, 17, { w: 1.8 }),
          el("div", { class: "body" }, el("span", { class: "ttl", text: "The hash chain does not verify" }), el("span", { class: "sub", text: `${L.break ? `Row ${n(L.break.row)} in ${L.break.store} is the first that does not match its parent` : "A row does not match its parent"} — rows were edited or removed after they were written. Totals below still count every row.` })),
          btn({ class: "btn danger", onclick: reverify }, "Re-verify"),
        ),
      );
    if (L.legacy_rows) parts.push(el("div", { class: "notice plain", text: "Some rows were written by an older semlith, one per open store, so totals that include them may count one search more than once. The chain is not rewritten to hide it." }));
    parts.push(
      el(
        "div",
        { class: "q4" },
        kpi("Queries recorded", n(L.queries), `${plural(L.clients || byClient.length, "client")}${lastRow ? ` · last ${ago(lastRow.at)}` : ""}`),
        kpi("Fewer tokens", L.ratio ? `${L.ratio.toFixed(1)}×` : "—", L.ratio ? `than reading those files whole · coverage ${L.coverage}% · ${L.tier}` : "counted once an agent asks", { warn: false }),
        kpi("Net tokens not read", n(L.net_tokens), "whole-file less excerpt, rows with a hit"),
        kpi("Zero-hit", `${zeroShare.toFixed(0)}%`, `${n(L.zero_hit)} queries the corpus could not answer`),
      ),
    );
    const whole = L.whole_file_tokens || 0;
    const sent = L.excerpt_tokens || 0;
    parts.push(
      el(
        "div",
        { class: "split s-12-1 stretch" },
        el(
          "div",
          { class: "card pad" },
          el("div", { class: "row base" }, el("span", { class: "card-t grow", text: "What agents read, against reading whole files" }), el("span", { class: "t-mono-sm", text: "store tokenizer · modelled" })),
          [
            ["Reading every file a retrieval answered from", whole, 100, "line"],
            ["What agents were actually sent", sent, whole ? Math.max(0.6, (sent / whole) * 100) : 0, "accent"],
          ].map(([k, v, p, tone]) => {
            const b = bar(p, `h10 ${tone === "accent" ? "accent" : ""}`);
            if (tone === "line") b.firstChild.style.background = "var(--line)";
            b.setAttribute("data-tip", k);
            b.setAttribute("data-tip-rows", rows([["tokens", n(v)], ["relative", `${p.toFixed(1)}%`], ["that is", p < 100 && L.ratio ? `${L.ratio.toFixed(1)}× fewer` : "the baseline"]]));
            return el("div", { class: "col gap4" }, el("div", { class: "row base t-sm" }, el("span", { class: "grow ink2", text: k }), el("span", { class: "mono t-b", text: n(v) })), b);
          }),
          el("div", { class: "row gap6 tb-line" }, [`coverage ${L.coverage || 0}%`, `refunds ${n(L.refunds)} · ${L.refunds_measured ? "measured" : "a floor"}`, `tier ${L.tier || "modelled"}`].map((f) => el("span", { class: "factchip", text: f }))),
        ),
        el(
          "div",
          { class: "card pad" },
          el("div", { class: "row base" }, el("span", { class: "card-t grow", text: "By client" }), el("span", { class: "t-mono-sm", text: "MCP, HTTP and CLI" })),
          byClient.length
            ? byClient.slice(0, 7).map(([k, v]) => {
                const b = bar((v / maxClient) * 100, lg.filter === k ? "accent" : "");
                return btn(
                  { class: `bar-row${lg.filter === k ? " on" : ""}`, "data-tip": k, "data-tip-rows": rows([["queries", n(v)], ["share", pct(v, L.queries)], ["click", lg.filter === k ? "clear the filter" : "filter the tables"]]), onclick: () => ((lg.filter = lg.filter === k ? null : k), repaint()) },
                  el("span", { class: "k ell", text: k }),
                  b,
                  el("span", { class: "right muted", text: n(v) }),
                );
              })
            : empty("No client has asked anything yet."),
        ),
      ),
    );
    // Usage from the clients' own logs: their tokens and cost beside ours.
    const usage = L.usage || {};
    const tabsNode = tabs(
      [
        ["sessions", "Sessions"],
        ["retrievals", "Retrievals"],
        ["replay", "Session replay"],
      ],
      lg.tab,
      (t) => go("ledger", t === "sessions" ? undefined : t),
      {
        cls: "in-card",
        extra: [
          el("span", { class: "spacer" }),
          lg.tab !== "replay"
            ? toggle(!!usage.enabled, "Usage from client logs", async (v) => {
                const out = await act(() => post("/api/ledger/usage", { on: v }), v ? "Reading each client's own usage logs" : "Usage from client logs off");
                if (out) await load("ledger", true), repaint();
              }, { cls: "t125", tip: "Fills the model and the real cost of each call from the client's own logs on this machine" })
            : null,
          lg.tab !== "replay" ? el("div", { class: "row gap6 pad7" }, ["Markdown", "CSV", "JSON"].map((x) => btn({ class: "btn xs", onclick: () => exportLedger(x) }, x))) : null,
        ],
      },
    );
    const card = el("div", { class: "card" }, tabsNode);
    if (lg.tab === "sessions") card.append(...ledgerSessions(sessionsAll.filter((s) => match(s.client))));
    else if (lg.tab === "retrievals") card.append(...ledgerRetrievals(rowsAll.filter((r) => match(r.client))));
    else card.append(ledgerReplay());
    parts.push(card);
    parts.push(
      el(
        "div",
        { class: "row" },
        el("span", { class: "muted t-sm", text: "Same thing in a terminal" }),
        [
          ["semlith ledger --last 20", "Prints the same rows this page shows"],
          ["semlith ledger --verify", "Re-walks the hash chain and names the first row that does not verify"],
          ["semlith start --no-ledger", "Run without recording, for this session only"],
        ].map(([c, tipText]) => btn({ class: "cmdchip", "data-tip": tipText, onclick: () => copy(c) }, c, icon(I.copy, 11, { w: 2 }))),
        el("span", { class: "spacer" }),
        el("span", { class: "t-mono-sm", text: "~/.semlith/stores/<name>/store.db · table retrievals" }),
      ),
    );
    return el("div", { class: "page" }, parts);
  },
};

async function reverify() {
  try {
    const out = await post("/api/ledger/verify", { repair: false });
    const broken = (out.stores || []).filter((s) => !s.intact);
    if (!broken.length) {
      toast("Re-walked every chain · intact");
    } else {
      const ok = await ask({
        title: "Re-anchor the chain?",
        body: `${broken.map((s) => `${s.store} breaks at row ${n(s.break_row)}`).join(" · ")}. A repair appends one note row saying so and verifies from there. No recorded row is edited or removed, so the break stays visible in the record.`,
        ok: "Append the note",
      });
      if (ok) {
        await post("/api/ledger/verify", { repair: true });
        toast("Chain re-anchored with a note row · nothing rewritten");
      }
    }
  } catch (e) {
    toast(e.message, true);
  }
  await load("ledger", true);
  paintChrome();
  repaint();
}

function storeSelect(value, onPick) {
  return dropdown({ label: "Store", value, options: [["all", "All stores"], ...liveStores().map((s) => [s.name, s.name])], onChange: onPick });
}

function clientSelect(rowsList) {
  const names = [...new Set(rowsList.map((r) => r.client))].sort();
  return dropdown({ label: "Client", value: lg.filter || "all", options: [["all", "All clients"], ...names.map((c) => [c, c])], onChange: (v) => ((lg.filter = v === "all" ? null : v), repaint()) });
}

function ledgerSessions(list) {
  const q = lg.q.toLowerCase();
  const all = data.ledger?.sessions || [];
  const filtered = list.filter((s) => (lg.store === "all" || s.store === lg.store) && (lg.tier === "all" || s.tier === lg.tier) && (!q || `${s.session} ${s.client}`.toLowerCase().includes(q)));
  const usage = data.ledger?.usage?.enabled;
  const priced = filtered.some((s) => s.saved_usd != null);
  const qInput = el("input", { value: lg.q, placeholder: "Filter by session or agent", "data-keep": "lg-q", "aria-label": "Filter sessions", oninput: (e) => ((lg.q = e.target.value), repaint()) });
  const g = grid({
    key: "lg:sessions",
    caption: "Ledger sessions",
    rows: filtered,
    sort: "seen",
    dir: "desc",
    empty: all.length ? "No session matches." : "No session recorded yet.",
    onClear: all.length ? clearLedger : null,
    columns: [
      { key: "seen", label: "Last seen", cls: "ms nowrap", firstDir: "desc", sort: (s) => s.last, render: (s) => el("span", { "data-tip": s.when, text: dayClock(s.last) }) },
      { key: "id", label: "Session", cls: "t-mono", sort: (s) => s.session, render: (s) => el("span", { "data-tip": s.session, text: String(s.session || "").slice(0, 10) }) },
      { key: "agent", label: "Agent", cls: "t13", sort: (s) => s.client, render: (s) => s.client },
      { key: "store", label: "Store", cls: "ms", sort: (s) => s.store, render: (s) => s.store },
      usage ? { key: "model", label: "Model", cls: "ms", sort: (s) => s.model || "", render: (s) => s.model || "—" } : null,
      { key: "reads", label: "Reads", cls: "m r", sort: (s) => s.retrievals, render: (s) => n(s.retrievals) },
      { key: "net", label: "Net tokens", cls: "m r", sort: (s) => s.net_tokens, render: (s) => n(s.net_tokens) },
      priced ? { key: "cost", label: "Saved", cls: "m r", sort: (s) => s.saved_usd || 0, render: (s) => (s.saved_usd != null ? el("span", { "data-tip": `at ${s.model}'s input price` }, dollars(s.saved_usd)) : "—") } : null,
      { key: "tier", label: "Tier", sort: (s) => s.tier, render: (s) => pill(s.tier, s.tier === "measured" ? "blue" : "grey", { dot: false, tip: s.tier === "measured" ? "Measured from the agent's own session log" : "Modelled with the store tokenizer, not observed" }) },
    ].filter(Boolean),
  });
  lg.shown = g.shown;
  return [
    el(
      "div",
      { class: "filterbar" },
      el("div", { class: "box h28 w220 full-sm" }, icon(I.searchSm, 13, { w: 1.8 }), qInput),
      clientSelect(all),
      storeSelect(lg.store, (v) => ((lg.store = v), repaint())),
      dropdown({
        label: "Tier",
        value: lg.tier,
        options: [
          ["all", "Any tier"],
          ["measured", "measured"],
          ["modelled", "modelled"],
        ],
        onChange: (v) => ((lg.tier = v), repaint()),
      }),
    ),
    g.node,
  ];
}

function ledgerRetrievals(list) {
  const q = lg.q.toLowerCase();
  const filtered = list.filter((r) => (lg.store === "all" || r.store === lg.store) && (!lg.zero || r.hits === 0) && (!q || String(r.query).toLowerCase().includes(q)));
  const qInput = el("input", { value: lg.q, placeholder: "Filter by query", "data-keep": "lg-q", "aria-label": "Filter retrievals", oninput: (e) => ((lg.q = e.target.value), repaint()) });
  const g = grid({
    key: "lg:retrievals",
    caption: "Ledger retrievals",
    rows: filtered,
    sort: "when",
    dir: "desc",
    empty: (data.ledger?.rows || []).length ? "No retrieval matches." : "No retrieval recorded yet.",
    onClear: (data.ledger?.rows || []).length ? clearLedger : null,
    columns: [
      { key: "when", label: "When", cls: "ms nowrap", firstDir: "desc", sort: (r) => r.at, render: (r) => el("span", { "data-tip": r.when, text: dayClock(r.at) }) },
      { key: "client", label: "Client", cls: "t13 nowrap", sort: (r) => r.client, render: (r) => r.client },
      { key: "store", label: "Store", cls: "ms", sort: (r) => r.store, render: (r) => r.store },
      { key: "q", label: "Query", cls: "m cap", sort: (r) => r.query, render: (r) => el("span", { "data-tip": r.query, "data-tip-full": "", text: r.query }) },
      { key: "hits", label: "Hits", cls: "m r", sort: (r) => r.hits, render: (r) => n(r.hits) },
      { key: "sent", label: "Sent", cls: "m r", sort: (r) => r.excerpt_tokens, render: (r) => `${n(r.excerpt_tokens)} tok` },
      { key: "ms", label: "ms", cls: "ms r", sort: (r) => r.ms, render: (r) => n(r.ms) },
    ],
  });
  lg.shown = g.shown;
  return [
    el(
      "div",
      { class: "filterbar" },
      el("div", { class: "box h28 w220 full-sm" }, icon(I.searchSm, 13, { w: 1.8 }), qInput),
      clientSelect(data.ledger?.rows || []),
      storeSelect(lg.store, (v) => ((lg.store = v), repaint())),
      btn({ class: "chip", "aria-pressed": String(lg.zero), onclick: () => ((lg.zero = !lg.zero), repaint()) }, "Zero-hit only"),
    ),
    g.node,
  ];
}

function clearLedger() {
  lg.filter = null;
  lg.q = "";
  lg.store = "all";
  lg.tier = "all";
  lg.zero = false;
  repaint();
}

const REPLAY_TONE = { refund: "amber", miss: "red", sufficed: "green", unknown: "grey" };

function ledgerReplay() {
  lg.shown = null;
  const r = data.replay || {};
  if (!r.enabled)
    return el(
      "div",
      { class: "card-b" },
      el(
        "div",
        { class: "row gap12" },
        el("span", { class: "grow muted t-sm", text: "Session replay reads this machine's agent session logs to show what an agent did after each answer. It is off. Local only — nothing is uploaded." }),
        btn({ class: "btn dark", onclick: () => setReplay(true) }, "Turn it on"),
      ),
    );
  const sessions = r.sessions || [];
  return el(
    "div",
    { class: "col" },
    el("div", { class: "card-note", text: `Read from ${r.client || "Claude Code"} transcripts under ${tilde(r.from || "")}${r.skipped ? ` · ${n(r.skipped)} older transcripts not read` : ""} · turn off on the Privacy page` }),
    !sessions.length ? empty(`On, and no transcript under ${tilde(r.from || "this machine")} holds a semlith call yet.`) : null,
    sessions.slice(0, 20).map((s) =>
      el(
        "div",
        { class: "card-b line-row gap8" },
        el(
          "div",
          { class: "row base nowrap t-mono" },
          el("span", { class: "t-m", text: String(s.id).slice(0, 12) }),
          el("span", { class: "muted grow ell", text: `${plural(s.answers, "answer")} · ${s.project}` }),
          el("span", { class: "muted", text: [s.refund && `${s.refund} refund`, s.miss && `${s.miss} miss`, s.sufficed && `${s.sufficed} sufficed`].filter(Boolean).join(" · ") }),
        ),
        (s.recent || []).slice(-6).map((a) =>
          el(
            "div",
            { class: "replay-item" },
            el("span", { class: "t-mono-sm", text: a.at ? new Date(a.at).toTimeString().slice(0, 8) : "" }),
            el("span", { class: "q" }, a.query || a.tool, " ", el("span", { class: "muted t-xs", text: a.query ? a.tool : "" })),
            pill(a.word, REPLAY_TONE[a.outcome] || "grey", { dot: false }),
          ),
        ),
      ),
    ),
  );
}

async function setReplay(on) {
  const out = await act(() => post("/api/ledger/replay", { on }), on ? "Session replay on · local logs only" : "Session replay off");
  if (out) {
    await load("replay", true);
    repaint();
  }
}

// A download's name without the model detail in brackets: the portal says
// "the embedding model", never which one.
function plainWhat(what) {
  return String(what || "").replace(/\s*\([^)]*\)/g, "");
}

function exportLedger(format) {
  const sessions = lg.tab === "sessions";
  const list = lg.shown ? lg.shown() : sessions ? data.ledger?.sessions || [] : data.ledger?.rows || [];
  const cols = sessions ? ["when", "session", "client", "store", "retrievals", "net_tokens", "tier", "model"] : ["when", "client", "store", "query", "hits", "excerpt_tokens", "whole_file_tokens", "ms"];
  const stamp = new Date().toISOString().slice(0, 10);
  const name = `ledger-${sessions ? "sessions" : "retrievals"}-${stamp}`;
  if (format === "JSON") return download(`${name}.json`, JSON.stringify(list, null, 2), "application/json");
  const cell = (v) => (v === null || v === undefined ? "" : String(v));
  // A Markdown table cell: backslashes first, then the pipe that would end
  // the cell, and a line break folded to a space so the row stays one row.
  const mdCell = (v) => cell(v).replace(/\\/g, "\\\\").replace(/\|/g, "\\|").replace(/\r?\n/g, " ");
  if (format === "CSV") {
    const esc = (v) => (/[",\n]/.test(cell(v)) ? `"${cell(v).replace(/"/g, '""')}"` : cell(v));
    return download(`${name}.csv`, [cols.join(","), ...list.map((r) => cols.map((c) => esc(r[c])).join(","))].join("\n"), "text/csv");
  }
  const md = [`# Retrieval ledger — ${sessions ? "sessions" : "retrievals"}`, "", `Exported ${new Date().toString()} from this machine.`, "", `| ${cols.join(" | ")} |`, `| ${cols.map(() => "---").join(" | ")} |`, ...list.map((r) => `| ${cols.map((c) => mdCell(r[c])).join(" | ")} |`)].join("\n");
  download(`${name}.md`, md, "text/markdown");
}

// ------------------------------------------------------------------- reports

const REPORTS = [
  ["savings", "Retrieval savings", "Tokens agents did not read, counted from the ledger.", "for whoever approves the spend", "What retrieval actually saved, per client, with the arithmetic shown."],
  ["access", "AI access audit", "Which agent read which file, when — from a hash-chained record.", "for security review", "Every file an agent was shown, by client, in the window."],
  ["change", "Change brief", "Blast radius for what changed, as a note to paste in the PR.", "for the reviewer", "What the change reaches."],
  ["health", "Index health", "Stale files, skipped formats and graph coverage.", "for whoever owns the store", "What the index holds, skipped, and let go stale."],
  ["gaps", "Knowledge gaps", "Questions the corpus could not answer well — a docs to-do list.", "for whoever writes the docs", "The questions that came back empty or thin."],
];
const REPORT_FORMATS = [
  ["markdown", "Markdown", "md", "text/markdown"],
  ["csv", "CSV", "csv", "text/csv"],
  ["json", "JSON", "json", "application/json"],
  ["html", "HTML", "html", "text/html"],
  ["pdf", "PDF", "pdf", "application/pdf"],
];
const REPORT_WINDOWS = [
  ["day", "24 hours"],
  ["week", "7 days"],
  ["month", "30 days"],
  ["quarter", "Quarter"],
];
const WINDOWED = ["access", "change"];
const CADENCES = [
  ["day", 86400],
  ["week", 604800],
  ["month", 2592000],
];

const rp = { kind: "savings", window: "month", format: "markdown", stores: [], hash: false, excerpts: false, model: "", out: null, busy: false, picking: false };

VIEWS.reports = {
  needs: () => ["stores", "prices", "schedules"],
  live: [],
  render(route, holder) {
    const prices = data.prices || {};
    const models = prices.savings_models || [];
    if (!rp.model || !models.some((m) => m.name === rp.model)) rp.model = (models.find((m) => /sonnet/i.test(m.name)) || models[0] || {}).name || "";
    const def = REPORTS.find((r) => r[0] === rp.kind);
    const inert = !WINDOWED.includes(rp.kind);
    const fmt = REPORT_FORMATS.find((f) => f[0] === rp.format);
    const fileName = `${rp.kind}-${new Date().toISOString().slice(0, 10)}.${fmt[2]}`;
    const preview = el("div", { class: "preview", "data-scroll-keep": "rp-preview" });
    const previewMeta = el("div", { class: "card-foot" });
    const savingsCard = el("div", {});
    holder.onData = () => {};
    const generate = async () => {
      // Only the latest request paints: a slow report finishing after a
      // quick one used to put the old answer back. Seconds tick meanwhile, so
      // a large store's report reads as working rather than stuck.
      const mine = (rp.seq = (rp.seq || 0) + 1);
      rp.busy = true;
      const began = Date.now();
      setText(preview, "Generating on this machine…");
      const tick = setInterval(() => {
        if (rp.seq !== mine || !preview.isConnected) return clearInterval(tick);
        setText(preview, `Generating on this machine… ${Math.round((Date.now() - began) / 1000)} s — a report over a large store reads all of it`);
      }, 1000);
      const p = new URLSearchParams({ kind: rp.kind, format: rp.format === "pdf" ? "markdown" : rp.format, model: rp.model });
      if (!inert) p.set("window", rp.window);
      for (const s of rp.stores) p.append("scope", s);
      if (rp.hash) p.set("redact", "1");
      if (rp.excerpts) p.set("excerpts", "1");
      try {
        const out = await api(`/api/report?${p}`);
        if (rp.seq !== mine) return;
        rp.out = out;
        setText(preview, rp.out.text || "");
        fill(previewMeta, `${def[4]} · ${rp.format} · ${rp.out.report ? rp.out.report.window : ""} · generated ${rp.out.generated || ""} · nothing uploaded`);
        paintSavings();
      } catch (e) {
        if (rp.seq !== mine) return;
        rp.out = null;
        setText(preview, e.message);
      } finally {
        clearInterval(tick);
        if (rp.seq === mine) rp.busy = false;
      }
    };
    const save = async () => {
      const p = new URLSearchParams({ kind: rp.kind, format: rp.format, model: rp.model });
      if (!inert) p.set("window", rp.window);
      for (const s of rp.stores) p.append("scope", s);
      if (rp.hash) p.set("redact", "1");
      if (rp.excerpts) p.set("excerpts", "1");
      try {
        const out = await api(`/api/report?${p}`);
        if (out instanceof Blob) download(fileName, out, fmt[3]);
        else download(fileName, out.text || "", fmt[3]);
        toast(`Saved ${fileName} through the browser`);
      } catch (e) {
        toast(e.message, true);
      }
    };
    function paintSavings() {
      if (rp.kind !== "savings" || !rp.out || !rp.out.report) return fill(savingsCard);
      const blocks = rp.out.report.blocks || [];
      const table = blocks.find((b) => b.block === "table");
      const facts = (blocks.find((b) => b.block === "facts") || {}).facts || [];
      const net = facts.find((f) => f[0] === "Net") || [];
      const netCost = facts.find((f) => /cost avoided/i.test(f[0])) || [];
      const model = models.find((m) => m.name === rp.model);
      fill(
        savingsCard,
        el(
          "div",
          { class: "card" },
          el(
            "div",
            { class: "card-h" },
            el("span", { class: "card-t grow", text: "The savings, line by line" }),
            el("span", { class: "eyebrow sm", text: "PRICED AT" }),
            // One dropdown with every model the savings can be priced at.
            dropdown({ label: "Price the savings at", value: rp.model, options: models.map((m) => [m.name, m.name, `$${m.input}/Mtok`]), width: 280, onChange: (v) => ((rp.model = v), generate()) }),
            el("span", { class: "t-mono-sm", text: model ? `$${Number(model.input).toFixed(2)} / Mtok input · prices from ${prices.source || "the built-in table"}${prices.fetched ? ` · ${prices.fetched}` : ""}` : "" }),
            btn({ class: "btn xs", onclick: updatePrices, "data-tip": `One request to ${prices.url || "models.dev"}, made now because you asked` }, "Update prices"),
          ),
          table
            ? table.rows.map((r) => el("div", { class: "save-line" }, el("span", { class: "col" }, el("span", { class: "t-m t-sm", text: r[0] }), el("span", { class: "muted t-xs", text: r[3] || "" })), el("span", { class: "tok", text: r[1] }), el("span", { class: "cost", text: r[2] || "—" })))
            : empty("No savings recorded yet."),
          el(
            "div",
            { class: "save-line net" },
            el("span", { class: "row gap6" }, el("span", { class: "t-b big14", text: "Net" }), facts.filter((f) => !/^Net/.test(f[0])).map((f) => el("span", { class: "factchip sm", text: `${f[0].toLowerCase()} ${f[1]}` }))),
            el("span", { class: "tok", text: net[1] || "—" }),
            el("span", { class: "cost", text: netCost[1] || "—" }),
          ),
        ),
      );
    }
    const optRow = (key, title, sub) => toggleRow(rp[key], title, sub, (v) => ((rp[key] = v), repaint(), generate()));
    const schedules = Object.entries((data.schedules || {}).schedules || {});
    const cli = `semlith report ${rp.kind} \\\n  --format ${rp.format} \\\n  ${inert ? "" : `--window ${rp.window} \\\n  `}--model "${rp.model}"${rp.stores.length ? ` \\\n  ${rp.stores.map((s) => `--store ${s}`).join(" ")}` : ""}${rp.hash ? " \\\n  --redact" : ""}${rp.excerpts ? " \\\n  --excerpts" : ""}`;
    const node = el(
      "div",
      { class: "page" },
      el("div", { class: "titles" }, el("div", { class: "h1", text: "Reports" }), el("div", { class: "lead", text: "Turn the ledger, index and graph into a file someone else can read. Generated here, saved through the browser, never uploaded." })),
      el(
        "div",
        { class: "split s-260" },
        el("div", { class: "col gap8" }, REPORTS.map(([id, title, desc, who]) => btn({ class: `kind-card${id === rp.kind ? " on" : ""}`, "aria-pressed": String(id === rp.kind), onclick: () => ((rp.kind = id), repaint()) }, el("span", { class: "t", text: title }), el("span", { class: "d", text: desc }), el("span", { class: "w", text: who })))),
        el(
          "div",
          { class: "stack" },
          el(
            "div",
            { class: "card pad auto-fit m320 gap14" },
            el("div", { class: "col gap6" }, el("span", { class: "eyebrow sm", text: "WINDOW" }), seg(REPORT_WINDOWS, inert ? "" : rp.window, (v) => ((rp.window = v), repaint()), { disabled: () => inert }), inert ? el("span", { class: "muted t-xs", text: "This report counts the whole history." }) : null),
            el("div", { class: "col gap6" }, el("span", { class: "eyebrow sm", text: "FORMAT" }), seg(REPORT_FORMATS.map((f) => [f[0], f[1]]), rp.format, (v) => ((rp.format = v), repaint()))),
            el(
              "div",
              { class: "col gap6 full-row" },
              el("span", { class: "eyebrow sm", text: "STORES" }),
              el(
                "div",
                { class: "row gap6" },
                btn({ class: "chip sm mono", "aria-pressed": String(!rp.stores.length), onclick: () => ((rp.stores = []), repaint()) }, "all stores"),
                liveStores().map((s) => btn({ class: "chip sm mono", "aria-pressed": String(rp.stores.includes(s.name)), onclick: () => ((rp.stores = rp.stores.includes(s.name) ? rp.stores.filter((x) => x !== s.name) : rp.stores.concat(s.name)), repaint()) }, s.name)),
              ),
            ),
            el("div", { class: "row gap8 full-row" }, optRow("hash", "Hash the query text", "Keeps who, when and which file; drops what was asked."), optRow("excerpts", "Attach the excerpts", "The exact lines each agent was shown. A larger file.")),
          ),
          el(
            "div",
            { class: "card flexcol" },
            el(
              "div",
              { class: "card-h tight" },
              icon(I.file, 14, { w: 1.6 }),
              el("span", { class: "mono t-m t-sm grow anywhere", text: fileName }),
              btn({ class: "btn sm t125", onclick: () => rp.out && copy(rp.out.text || "", "Report copied") }, "Copy"),
              btn({ class: "btn sm t125", onclick: () => ((rp.picking = true), repaint()) }, "Schedule…"),
              btn({ class: "btn sm primary t125", onclick: save }, "Save to disk"),
            ),
            preview,
            previewMeta,
          ),
          savingsCard,
        ),
      ),
      el(
        "div",
        { class: "split s-1-1 stretch" },
        el(
          "div",
          { class: "card flexcol" },
          el("div", { class: "card-h" }, el("span", { class: "card-t grow", text: "Schedules" }), el("span", { class: "t-mono-sm", text: "run by the daemon · ~/.semlith/schedules.json" })),
          !schedules.length && !rp.picking ? empty("None yet. Set a report up above and press Schedule — the daemon writes the file in place, so it can be a tracked file with a readable diff.") : null,
          schedules.map(([id, s]) =>
            el(
              "div",
              { class: "sched-row" },
              el("span", { class: "col" }, el("span", { class: "t-m t-sm", text: (REPORTS.find((r) => r[0] === s.kind) || [, s.kind])[1] }), el("span", { class: "txt-dim", text: `${tilde(s.dir)} · ${s.format}${s.last_error ? ` · last run failed: ${s.last_error}` : s.next_run ? ` · next ${until(s.next_run)}` : ""}` })),
              pill(everyWord(s.every_seconds), s.last_error ? "red" : "blue", { dot: false }),
              btn({ class: "x-btn", "aria-label": "Remove schedule", onclick: () => removeSchedule(id) }, icon(I.x, 12, { w: 2 })),
            ),
          ),
          rp.picking
            ? el(
                "div",
                { class: "card-foot sans" },
                el("span", { class: "ink2 t-sm", text: "Every" }),
                CADENCES.map(([label, secs]) => btn({ class: "chip sm", onclick: () => addSchedule(secs) }, label)),
                el("span", { class: "spacer" }),
                lnk("Cancel", () => ((rp.picking = false), repaint()), "muted"),
              )
            : null,
        ),
        el(
          "div",
          { class: "card flexcol" },
          el("div", { class: "card-h" }, el("span", { class: "card-t grow", text: "Without the browser" }), lnk("Copy", () => copy(cli.replace(/\\\n\s*/g, "")))),
          el("div", { class: "code flat wrap grow", text: cli }),
          el("div", { class: "card-note" }, "An agent can ask too: ", el("span", { class: "mono ink2", text: "semlith_report" }), " returns the same document over MCP."),
        ),
      ),
    );
    setTimeout(generate, 0);
    return node;
  },
};

function until(unix) {
  const s = unix - Math.floor(Date.now() / 1000);
  if (s <= 0) return "due now";
  if (s < 3600) return `in ${Math.round(s / 60)}m`;
  if (s < 86400) return `in ${Math.round(s / 3600)}h`;
  return `in ${Math.round(s / 86400)}d`;
}

function everyWord(seconds) {
  const hit = CADENCES.find(([, s]) => s === seconds);
  if (hit) return `every ${hit[0]}`;
  if (seconds % 86400 === 0) return `every ${seconds / 86400} days`;
  return `every ${Math.round(seconds / 3600)} h`;
}

async function addSchedule(every) {
  const dir = await pickFolder({ title: "Where should the daemon write it?", ok: "Write it here", hint: "The file is replaced in place on each run, so a tracked folder shows a readable diff." });
  if (!dir) return;
  const out = await act(() => post("/api/schedules", { action: "add", kind: rp.kind, format: rp.format, model: rp.model, every_seconds: every, dir, window: WINDOWED.includes(rp.kind) ? rp.window : undefined, stores: rp.stores }), `Scheduled ${everyWord(every)}`);
  rp.picking = false;
  if (out) await load("schedules", true);
  repaint();
}

async function removeSchedule(id) {
  const out = await act(() => post("/api/schedules", { action: "remove", id }), "Schedule removed");
  if (out) await load("schedules", true), repaint();
}

async function updatePrices() {
  const ok = await ask({ title: "Update the price table?", body: `One request to ${data.prices?.url || "models.dev"}, made now because you asked. ${data.privacy?.airgap?.on || data.privacy?.airgap === true ? "Airgap is on, so it will be refused." : "Nothing else is sent."}`, ok: "Update" });
  if (!ok) return;
  const out = await act(() => post("/api/prices", { update: true }), (o) => `Prices updated · ${o.models} models`);
  if (out) await loadMany(["prices", "privacy"], true), repaint();
}

// ------------------------------------------------------------------- privacy

const pvUi = { done: {}, scan: null, scanning: false };

// Each rule in a line, as the design writes them, and its finding in a few
// words. A rule that does not hold keeps the daemon's full wording, because
// then the detail is the point; the full finding is always on hover.
const RULE_COPY = {
  "header-borne token": ["The session token travels in a Semlith-Token header, never a cookie — every localhost port is the same site.", () => "no route sets or reads a cookie"],
  "same-origin writes": ["Every non-GET carries Sec-Fetch-Site: same-origin and a JSON content type, checked before the token.", () => "enforced before any route runs"],
  "refused-file acceptance": ["A person decides about refused files, one or several at once — never an agent.", () => "the decision routes take the session token only"],
  "store trust": ["A store outside the store home opens only after semlith trust records it.", (c) => (/no store/.test(c) ? "no untrusted store open" : shortPaths(c))],
  "index boundary": ["An agent indexes only under a store's registered roots or home.", () => "enforced per path"],
  "deny-list": ["No credential directory or credential-named file is indexed by an agent or the portal.", (c) => c.replace(/ and /, " · ").replace(/name patterns/, "patterns")],
  "private addresses": ["semlith add refuses loopback, RFC 1918, link-local and unique-local.", (c) => c],
  "pinned models": ["Every model file is checked against a pinned hash before it loads.", () => "pinned hash matches"],
  "model cache": ["Weights load only from a cache no other account owns or can write to.", () => "yours alone"],
  "directory modes": ["The store home, every store, the model cache and daemon.json are readable by you alone.", (c) => (/0700/.test(c) ? "all 0700" : shortPaths(c))],
  "agent key": ["The agent key is one file, readable by you alone; nothing writes it into a client's config.", (c) => (/600/.test(c) ? "600, owner only" : shortPaths(c))],
};

VIEWS.privacy = {
  needs: () => ["privacy", "replay", "stores"],
  live: ["privacy"],
  render() {
    savePrefs({ privacySeen: true });
    const P = data.privacy || {};
    const airgap = P.airgap && typeof P.airgap === "object" ? P.airgap : { on: !!P.airgap, reason: P.airgap ? "flag" : null };
    const outbound = P.outbound || { count: 0, recent: [] };
    const downloads = P.downloads || [];
    const cached = downloads.filter((d) => d.cached);
    const replayOn = !!(data.replay && data.replay.enabled);
    const rules = P.rules || [];
    const holding = rules.every((r) => r.ok !== false);
    const left = outbound.count > 0;
    const verifyCmds = [
      ["Ask the operating system what this process has open. Only loopback should appear.", "lsof -nP -p $(pgrep -f 'semlith start') -i"],
      ["Watch every interface but loopback while you search. Nothing should appear.", "sudo tcpdump -i any -n 'not host 127.0.0.1 and not host ::1'"],
      ["Arm the refusal: anything that would reach the network exits instead, naming it.", "semlith start --airgap"],
      ["Or pull the cable. The portal loads and searches with no network at all.", "networksetup -setairportpower en0 off"],
      ["See the ledger for what it is: one local table.", "sqlite3 ~/.semlith/stores/<name>/store.db 'select * from retrievals'"],
    ];
    const nDone = verifyCmds.filter((_, i) => pvUi.done[i]).length;
    return el(
      "div",
      { class: "page" },
      el("div", { class: "titles" }, el("div", { class: "h1", text: "Privacy" }), el("div", { class: "lead", text: "The claim is “nothing leaves this machine”. This page is how you check it yourself, in about a minute." })),
      el(
        "div",
        { class: `verdict${left ? " amber" : ""}` },
        el("span", { class: "ic" }, icon(I.shield, 18, { w: 1.8 })),
        el(
          "div",
          { class: "col gap2 grow" },
          el("span", { class: "t", text: left ? `${plural(outbound.count, "request")} left this machine since start` : "Nothing has left this machine" }),
          el("span", { class: "l", text: `${n(outbound.count)} outbound connections since ${outbound.since ? clock(outbound.since) : "start"}${outbound.recent && outbound.recent.length ? ` · last: ${outbound.recent[0].what} to ${outbound.recent[0].host}` : ""} · ${cached.length ? `${plural(cached.length, "download")} on disk: ${cached.map((d) => `${plainWhat(d.what).replace(/^the /, "")} ${bytes(d.bytes)}`).join(", ")}` : "nothing downloaded"}${airgap.on ? " · airgap on" : ""}` }),
        ),
        toggle(airgap.on, "Airgap · refuse every outbound request", async (v) => {
          const out = await act(() => post("/api/airgap", { on: v }), v ? "Airgap on — any outbound request exits and names itself" : "Airgap off");
          if (out) await load("privacy", true), repaint();
        }, { disabled: airgap.reason === "flag" || airgap.reason === "env", tip: airgap.reason === "flag" || airgap.reason === "env" ? "Set by the flag or SEMLITH_AIRGAP this daemon started with" : null }),
      ),
      el(
        "div",
        { class: "auto-fit m200" },
        [
          ["Bind address", P.bind || location.host, "loopback only, no flag to change it"],
          ["CORS", P.cors ? "on" : "none", "no origin may read a response"],
          ["Host check", "localhost only", `a foreign Host header gets 400 · ${(P.host_allowed || []).join(", ")}`],
          ["Telemetry", "none", "no analytics, no update check of its own"],
          ["Assets", "inside the binary", "this page is compiled in, include_bytes!"],
          ["Ledger", "local table", "in each store's own database"],
          ["Model cache", shortPath(P.model_cache || "", 30), P.model_cached ? "read once, then offline" : "empty until the first run"],
          cloudFact(P.cloud),
        ].map(([k, v, d]) => el("div", { class: "fact-card" }, el("span", { class: "eyebrow sm", text: k }), el("span", { class: "v", text: v }), el("span", { class: "d", text: d }))),
      ),
      el(
        "div",
        { class: "split s-1-1" },
        el(
          "div",
          { class: "card" },
          el("div", { class: "card-h" }, el("span", { class: "card-t grow", text: "Verify it yourself" }), meta(nDone ? `${nDone} of 5 checked` : "tick each one off as you go")),
          verifyCmds.map(([t, c], i) =>
            el(
              "div",
              { class: "verify-row" },
              btn({ class: `verify-dot${pvUi.done[i] ? " done" : ""}`, "aria-pressed": String(!!pvUi.done[i]), "aria-label": `Mark step ${i + 1} checked`, onclick: () => ((pvUi.done[i] = !pvUi.done[i]), repaint()) }, pvUi.done[i] ? "✓" : String(i + 1)),
              el("div", { class: "col gap6" }, el("span", { class: "t13", text: t }), copyField(c, { btn: "xxs" })),
            ),
          ),
        ),
        el(
          "div",
          { class: "stack" },
          el(
            "div",
            { class: "card" },
            el("div", { class: "card-h" }, el("span", { class: "card-t grow", text: "Everything semlith ever fetches" }), meta(`${downloads.length + 2} things, each on your say-so`)),
            downloads.map((d) => el("div", { class: "dl-row" }, el("span", { class: "col" }, el("span", { class: "t-m t-sm", text: cap(plainWhat(d.what)) }), el("span", { class: "muted t-xs", text: `${d.source} · ${bytes(d.bytes)} · ${d.when}` })), pill(d.cached ? "on disk" : "never fetched", d.cached ? "green" : "grey", { dot: false }))),
            el("div", { class: "dl-row" }, el("span", { class: "col" }, el("span", { class: "t-m t-sm", text: "A URL you add to a store" }), el("span", { class: "muted t-xs", text: "one https request for exactly that URL · only when you press Fetch" })), pill("on request", "grey", { dot: false })),
            el("div", { class: "dl-row" }, el("span", { class: "col" }, el("span", { class: "t-m t-sm", text: "Semlith Cloud" }), el("span", { class: "muted t-xs", text: P.cloud?.signed_in ? `${(P.cloud.orgs || []).map((o) => o.host_name).join(", ")} · a search naming a remote store, the ledger rows of the stores you sync, and what you push or send · never the token anywhere else` : "nothing, until you run semlith cloud login" })), pill(P.cloud?.signed_in ? "signed in" : "never contacted", P.cloud?.signed_in ? "blue" : "grey", { dot: false })),
            el("div", { class: "dl-row" }, el("span", { class: "col" }, el("span", { class: "t-m t-sm", text: "The release check and the price table" }), el("span", { class: "muted t-xs", text: "github.com and models.dev · only when you press the button in Settings or Reports" })), pill("on request", "grey", { dot: false })),
            outbound.recent && outbound.recent.length ? el("div", { class: "card-foot", text: `last ${outbound.recent.length}: ${outbound.recent.slice(0, 4).map((r) => `${clock(r.at)} ${r.what} → ${r.host}`).join(" · ")}` }) : null,
          ),
          el(
            "div",
            { class: "card pad" },
            el("div", { class: "row" }, el("span", { class: "card-t grow", text: "What the stores already hold" }), btn({ class: "btn sm t125", disabled: pvUi.scanning ? true : null, onclick: scanStores }, pvUi.scan ? "Check again" : "Check now")),
            el("span", { class: "muted t-sm pretty", text: "Checks for any file the rules would refuse today — indexed before a rule widened, or before the credential scan existed." }),
            pvUi.scanning ? el("div", { class: "row gap8 muted t-sm" }, el("span", { class: "spinner" }), "Reading every store…") : null,
            pvUi.scan && !pvUi.scanning
              ? pvUi.scan.error
                ? errorBox(pvUi.scan.error)
                : pvUi.scan.findings.length
                  ? el(
                      "div",
                      { class: "col gap4" },
                      el("div", { class: "notice amber" }, el("span", { class: "sub", text: `${plural(pvUi.scan.findings.length, "file")} the rules would refuse today` })),
                      pvUi.scan.findings.slice(0, 20).map((f) => el("div", { class: "row nowrap" }, el("span", { class: "row gap6 grow min0 mono t-xs", "data-tip": f.why }, el("span", { class: "nowrap", text: `${f.store} ·` }), pathSpan(tilde(f.path), "min0", f.path)), lnk("Forget", () => forgetFound(f), "amber"))),
                    )
                  : el("div", { class: "notice green" }, el("span", { class: "dot green" }), `${plural(liveStores().length, "store")} checked · 0 files the rules would refuse today`)
              : null,
          ),
          toggleRow(replayOn, `Session replay · ${replayOn ? "on" : "off"}`, "Reads this machine's agent session logs to confirm what an agent did after a semlith answer. Local only. Nothing is uploaded.", (v) => setReplay(v), { big: true }),
        ),
      ),
      el(
        "div",
        { class: "card" },
        el("div", { class: "card-h" }, el("span", { class: "card-t grow", text: "Rules the binary enforces" }), pill(holding ? "all holding" : `${rules.filter((r) => r.ok === false).length} to fix`, holding ? "green" : "amber")),
        el(
          "div",
          { class: "auto-fill m300" },
          rules.map((r) =>
            el(
              "div",
              { class: "rule" },
              el("div", { class: "t" }, dot(r.ok === false ? "amber" : "green"), cap(r.id)),
              el("span", { class: "d", text: r.ok === false ? r.rule.replace(/\s+/g, " ") : (RULE_COPY[r.id] || [r.rule])[0].replace(/\s+/g, " ") }),
              el("span", { class: "f", "data-tip": r.check, text: `found: ${r.ok === false ? shortPaths(r.check) : (RULE_COPY[r.id] && RULE_COPY[r.id][1] ? RULE_COPY[r.id][1](r.check) : shortPaths(r.check))}` }),
              r.ok === false && r.repair ? btn({ class: "btn xs", onclick: () => fixRule(r.id) }, "Repair") : r.ok === false && r.manual ? copyField(r.manual) : null,
            ),
          ),
        ),
      ),
    );
  },
};

function cap(s) {
  const t = String(s || "");
  return t.charAt(0).toUpperCase() + t.slice(1);
}

async function scanStores() {
  pvUi.scanning = true;
  repaint();
  try {
    pvUi.scan = await api("/api/privacy/scan");
  } catch (e) {
    pvUi.scan = { error: e.message, findings: [] };
  }
  pvUi.scanning = false;
  repaint();
}

async function forgetFound(f) {
  const ok = await ask({ title: `Forget ${baseName(f.path)}?`, body: `${f.why}. Its chunks leave ${f.store}; the file on disk is untouched.`, ok: "Forget", danger: true });
  if (!ok) return;
  const out = await act(() => post("/api/forget", { store: f.store, paths: [f.key || f.path] }), "Forgotten");
  if (out) scanStores();
}

async function fixRule(id) {
  const out = await act(() => post("/api/privacy/fix", { rule: id }), "Repaired");
  if (out) await load("privacy", true), repaint();
}

// ------------------------------------------------------------------ settings

VIEWS.settings = {
  needs: (route) => {
    const sec = route.parts[0] || "perf";
    return sec === "perf" ? ["runs", "accel"] : sec === "access" ? ["about", "agents", "privacy"] : sec === "about" ? ["about", "languages", "prices"] : sec === "cloud" ? ["about", "cloud", "stores", "replay"] : ["about"];
  },
  live: ["runs"],
  morph: (route) => (route.parts[0] || "perf") === "perf",
  // Each limit's reason quotes free memory, which moves every poll; what is
  // drawn changes only when a value or where it came from does.
  sig: () => {
    const L = data.runs?.limits || {};
    const set = Object.entries(L).filter(([k]) => k !== "machine").map(([k, v]) => [k, v && v.value, v && v.source]);
    return JSON.stringify([set, data.runs?.compaction, data.runs?.vector_cache?.cap_mb, (data.accel?.lanes || []).map((l) => [l.lane, l.enabled])]);
  },
  render(route) {
    const sec = route.parts[0] || "perf";
    const machine = data.runs?.limits?.machine;
    const sections = [
      ["perf", "Performance", machine ? `${machine.logical_cores} cores` : ""],
      ["access", "Agent access", "key · token"],
      ["cloud", "Cloud", (data.about?.cloud?.orgs || []).map((o) => o.org).join(", ") || "off"],
      ["about", "About", state.version],
    ];
    const body = { perf: sePerf, access: seAccess, cloud: seCloud, about: seAbout }[sec] || sePerf;
    return el(
      "div",
      { class: "page" },
      el("div", { class: "titles" }, el("div", { class: "h1", text: "Settings" }), el("div", { class: "lead", text: "How hard this machine works, who may reach the endpoint, and what is installed." })),
      el(
        "div",
        { class: "split s-200" },
        el("div", { class: "subnav" }, sections.map(([id, label, m]) => btn({ "aria-current": String(id === sec), onclick: () => go("settings", id === "perf" ? undefined : id) }, el("span", { class: "grow", text: label }), el("span", { class: "meta", text: m })))),
        el("div", { class: "stack" }, body()),
      ),
    );
  },
};

// A stepper button that reaches its end says so with aria-disabled rather than
// disabled: a disabled button drops the focus a keyboard user just gave it.
// The processor, as the CPU lane names it ("Apple M1").
function cpuName() {
  return ((data.accel?.lanes || []).find((l) => l.lane === "cpu") || {}).device || "";
}

function stepBtn(off, label, onclick, text) {
  return btn({ "aria-disabled": off ? "true" : "false", "aria-label": label, onclick: () => !off && onclick() }, text);
}

async function saveLimits(patch, word) {
  // The daemon says what it now runs with; that is the message, when it says.
  const out = await act(() => post("/api/index/settings", patch), (o) => (o && o.applied) || word || "Saved");
  if (out) await load("runs", true), repaint();
}

function sePerf() {
  const R = data.runs || {};
  const L = R.limits || {};
  const m = L.machine || {};
  const comp = R.compaction || {};
  const vc = R.vector_cache || {};
  const limit = (key, label, lim, note, step, unit) => {
    if (!lim) return null;
    const env = lim.source === "set by the environment";
    return el(
      "div",
      { class: "limit-row" },
      el("span", { class: "col gap2" }, el("span", { class: "k", text: label }), el("span", { class: "n", text: `${note}${lim.derived !== lim.value ? ` This machine suggests ${n(lim.derived)}${unit || ""}.` : ""}${env ? " Set by the environment, so the page cannot change it." : ""}` })),
      el(
        "div",
        { class: "stepper", "data-tip": lim.reason || null },
        stepBtn(env || lim.value <= 1, `Lower ${label}`, () => saveLimits({ [key]: Math.max(1, lim.value - step) }), "−"),
        el("span", { class: "v", text: `${n(lim.value)}${unit || ""}` }),
        stepBtn(env || lim.value >= lim.ceiling, `Raise ${label}`, () => saveLimits({ [key]: Math.min(lim.ceiling, lim.value + step) }), "+"),
      ),
    );
  };
  const plain = (key, label, value, note, step, min, max, unit) =>
    el(
      "div",
      { class: "limit-row" },
      el("span", { class: "col gap2" }, el("span", { class: "k", text: label }), el("span", { class: "n", text: note })),
      el(
        "div",
        { class: "stepper" },
        stepBtn(value <= min, `Lower ${label}`, () => saveLimits({ [key]: Math.max(min, value - step) }), "−"),
        el("span", { class: "v", text: `${value === 0 ? "off" : `${n(value)}${unit || ""}`}` }),
        stepBtn(value >= max, `Raise ${label}`, () => saveLimits({ [key]: Math.min(max, value + step) }), "+"),
      ),
    );
  const lanes = data.accel?.lanes || [];
  return [
    el(
      "div",
      { class: "model-line t-mono ink2" },
      el("span", { class: "eyebrow sm", text: "THIS MACHINE" }),
      m.logical_cores ? `${cpuName() ? `${cpuName()} · ` : ""}${m.logical_cores} logical cores · ${n(m.total_memory_mb)} MiB · ${n(m.available_memory_mb)} MiB free now` : "reading…",
      el("span", { class: "spacer" }),
      lnk("Reset to what it suggests", () => saveLimits({ runs_at_once: L.runs_at_once?.derived, embed_threads: L.embed_threads?.derived, index_memory_mb: L.index_memory_mb?.derived, compact_threshold_percent: comp.default_threshold_percent, history_retention_days: comp.default_retention_days, vector_cache_mb: vc.default_cap_mb }, "Limits reset to what this machine suggests")),
    ),
    el(
      "div",
      { class: "card" },
      el("div", { class: "card-h" }, el("span", { class: "card-t", text: "Limits" })),
      limit("runs_at_once", "Runs at once", L.runs_at_once, "How many stores may index at the same time.", 1),
      limit("embed_threads", "Threads per run", L.embed_threads, "Embedding threads, with one core kept free for you.", 1),
      limit("index_memory_mb", "Memory per store", L.index_memory_mb, "Vectors held in memory per open store. Lower it if other apps feel slow.", 64, " MiB"),
      plain("compact_threshold_percent", "Compact past", comp.threshold_percent ?? 25, "An idle store more reclaimable than this is compacted on its own. 0 turns it off.", 5, 0, 90, "%"),
      plain("history_retention_days", "Keep retired definitions", comp.retention_days ?? 90, "How long a compaction keeps the history of a symbol that was renamed or deleted. 0 keeps everything.", 15, 0, 3650, " days"),
      vc.from_environment
        ? el("div", { class: "limit-row" }, el("span", { class: "col gap2" }, el("span", { class: "k", text: "Vector cache" }), el("span", { class: "n", text: `Set by SEMLITH_VECTOR_CACHE_MB · ${cacheWord(vc)}` })), el("span", { class: "mono right", text: `${n(vc.cap_mb)} MiB` }))
        : plain("vector_cache_mb", "Vector cache", vc.cap_mb ?? 1024, `Vectors kept so a chunk met again is not re-embedded · ${cacheWord(vc)}.`, 256, 0, 16384, " MiB"),
    ),
    el(
      "div",
      { class: "card" },
      el("div", { class: "card-h" }, el("span", { class: "card-t grow", text: "Where embedding runs" }), btn({ class: "btn sm t125", onclick: checkLanes }, seUi.gpu ? "Run the check again" : "Check each lane against the CPU")),
      data.accel?.cpu_fallback ? el("div", { class: "card-note", text: "The CPU is carrying the work: no other lane can." }) : null,
      // Lanes this machine can run first; the ones built for another OS or
      // hardware after them, under their own line, switched off and said so
      // in a few words rather than a row that reads like the others.
      [...lanes.filter((l) => l.status?.state !== "unavailable"), ...lanes.filter((l) => l.status?.state === "unavailable")].map((l, i, all) => {
        const na = ["unavailable"].includes(l.status?.state);
        const firstNa = na && (i === 0 || all[i - 1].status?.state !== "unavailable");
        const check = seUi.gpu && (seUi.gpu.checks || []).find((x) => x.lane === l.lane);
        const row = el(
          "div",
          { class: `lane-row${na ? " off na" : ""}`, "aria-disabled": na ? "true" : null },
          btn({ class: `switch${l.enabled ? " on" : ""}`, role: "switch", "aria-checked": String(!!l.enabled), "aria-label": `${l.label || l.lane} lane`, disabled: na ? true : null, onclick: () => laneSwitch(l) }, el("span", { class: "tg" })),
          el(
            "span",
            { class: "col" },
            el("span", { class: "row gap8 t-m t13" }, `${l.label || laneName(l.lane)}${l.variant ? ` · ${l.variant}` : ""}`, l.experimental ? el("span", { class: "exp", text: "experimental" }) : null),
            na
              ? el("span", { class: "muted t-xs", text: `Not available on this ${osWord()}: ${String(l.status?.reason || "").replace(/^the .*? lane (is )?/i, "").replace(/^unavailable — /, "")}` })
              : el("span", { class: "muted t-xs", text: `${l.device || "named when it starts"} · ${laneState(l.status)}${(data.accel.bytes || {})[l.lane] ? ` · ${bytes(data.accel.bytes[l.lane])} on disk` : ""}` }),
          ),
          na ? el("span", { class: "t-mono-sm muted right nowrap", text: "off" }) : check ? laneCheck(check) : el("span", { class: "t-mono-sm ink2 right nowrap", text: `${Math.round(l.share || 0)}% of the work` }),
          // Its own column at the far right, kept on every row, so the share
          // column lines up whether or not a lane has files to remove.
          el("span", { class: "lane-act" }, (data.accel.bytes || {})[l.lane] ? btn({ class: "btn xs", "aria-label": `Remove ${l.label || laneName(l.lane)}'s downloaded files`, onclick: () => laneRemove(l) }, "Remove") : null),
        );
        return firstNa ? [el("div", { class: "lane-group", text: `Not for this ${osWord()}` }), row] : row;
      }),
    ),
  ];
}

const seUi = { gpu: null, reveal: null, update: null };

function cacheWord(vc) {
  if (!vc.cap_mb) return "off";
  const used = vc.bytes ? `${n(vc.bytes / 1048576)} of ${n(vc.cap_mb)} MiB used` : "empty";
  return `${used}${vc.lookups ? ` · ${Math.round((vc.hits * 100) / vc.lookups)}% hit` : ""}`;
}

function laneState(status) {
  const st = (status && status.state) || "idle";
  const p = status && typeof status.percent === "number" ? ` ${status.percent}%` : "";
  const left = status && typeof status.eta_ms === "number" ? ` · ${spellLeft(status.eta_ms)}` : "";
  if (st === "compiling") return `compiling${p}${left} — first time only, runs wait for it`;
  if (st === "downloading") return `downloading${p}${left}`;
  if (status && status.reason) return `${st} — ${status.reason}`;
  return `${st}${p}`;
}

async function laneSwitch(l) {
  const on = !l.enabled;
  if (on && l.download_bytes && !l.installed) {
    const ok = await ask({ title: `Turn ${l.label || l.lane} on?`, body: `It downloads the ${l.label || l.lane} pack first, ${bytes(l.download_bytes)}, once, into this machine's model cache.${l.experimental ? " It is experimental: built and checked without its hardware." : ""}`, ok: "Download and turn on" });
    if (!ok) return;
  }
  const out = await act(() => post("/api/accel", { lane: l.lane, action: on ? "on" : "off" }), (o) => o.said || `${l.label || l.lane} ${on ? "on" : "off"}`);
  if (out) await load("accel", true), repaint();
}

// A lane's downloaded files, taken off the disk. A lane that is on is turned
// off first: its files are in use, and the CPU (or another lane) carries on.
async function laneRemove(l) {
  const name = l.label || laneName(l.lane);
  const size = bytes((data.accel.bytes || {})[l.lane] || 0);
  const ok = await ask({
    title: `Remove ${name}'s files?`,
    body: `${l.enabled ? `${name} is turned off first, and the other lanes carry the work. ` : ""}${size} comes off the disk. Turning it on again downloads it again.`,
    ok: "Remove",
    danger: true,
  });
  if (!ok) return;
  if (l.enabled && !(await act(() => post("/api/accel", { lane: l.lane, action: "off" }), null))) return;
  const out = await act(() => post("/api/accel", { lane: l.lane, action: "remove" }), (o) => o.said || `${name}'s files removed`);
  if (out || l.enabled) await load("accel", true), repaint();
}

// "Mac", "Windows" or "Linux", for the lanes this machine cannot run.
function osWord() {
  const ua = navigator.userAgent || "";
  return /Mac/.test(ua) ? "Mac" : /Windows/.test(ua) ? "Windows PC" : /Linux|X11/.test(ua) ? "Linux machine" : "machine";
}

// One lane's answer from the known-answer check, where its share sits: it
// agrees with the reference (cosine, speed), it failed, or why it was not run.
function laneCheck(c) {
  if (c.passed) return el("span", { class: "t-mono-sm right nowrap ok-ink", "data-tip": `Embeds the known sentences as the reference does · ${c.variant || ""}` }, `agrees · cosine ${Number(c.cosine).toFixed(4)}${c.chunks_per_s ? ` · ${perSecond(c.chunks_per_s)}/s` : ""}`);
  if (c.passed === false) return el("span", { class: "t-mono-sm right nowrap red-ink", "data-tip": c.reason || "" }, "failed");
  const why = String(c.reason || "").replace(/ — .*$/, "");
  return el("span", { class: "t-mono-sm muted right nowrap", "data-tip": c.reason || "" }, why === "off" ? "off · not checked" : "not checked");
}

// Every lane that is on embeds a few known sentences and is compared with
// the CPU's answer. A lane's first check starts its worker, which can take a
// while on a cold machine, so the button spins and the toast says so.
async function checkLanes() {
  toast("Checking each lane that is on against the CPU — a few seconds per lane");
  try {
    seUi.gpu = await post("/api/doctor/gpu", {});
    const ran = (seUi.gpu.checks || []).filter((c) => c.passed !== undefined);
    const bad = ran.filter((c) => !c.passed);
    toast(bad.length ? `${plural(bad.length, "lane")} failed: ${bad.map((c) => c.label || c.lane).join(", ")}` : `${plural(ran.length, "lane")} checked — every one agrees with the CPU`, bad.length > 0);
  } catch (e) {
    toast(e.message, true);
  }
  await load("accel", true);
  repaint();
}

// A client's note, as its first sentence: the rest (file formats, flags,
// where the source says so) is a press away, not a wall of text by default.
function clientNote(note) {
  const first = (note.match(/^.*?[.!?](?=\s|$)/) || [note])[0];
  if (first.length >= note.length - 2) return el("span", { class: "muted t-xs", text: note });
  const open = ag.noteOpen === note;
  return el("span", { class: "muted t-xs" }, open ? note : first, " ", lnk(open ? "Less" : "More", () => ((ag.noteOpen = open ? null : note), repaint())));
}

function seAccess() {
  const a = data.agents || {};
  const P = data.privacy || {};
  const about = data.about || {};
  const login = about.login || {};
  const keyValue = seUi.reveal || (a.key_set ? "sml_••••••••••••••••••••••" : "not created yet");
  return [
    el(
      "div",
      { class: "card pad" },
      el("span", { class: "card-t", text: "Agent key" }),
      el(
        "div",
        { class: "copyfield h34" },
        el("span", { class: "t", text: keyValue }),
        btn({ class: "btn xs", onclick: revealKey }, seUi.reveal ? "Hide" : "Reveal"),
        seUi.reveal ? copyBtn(seUi.reveal, "Copy", "xs") : null,
        btn({ class: "btn xs", onclick: rotateKey }, "Rotate"),
      ),
      el("span", { class: "muted t-sm pretty", text: `Only the HTTP form needs it. A registered client launches semlith mcp, which reads ${shortPath(a.key_path || "~/.semlith/agent.key", 40)} itself, so rotating reconfigures nothing.${P.key_grace_seconds ? ` The previous key keeps working for ${Math.ceil(P.key_grace_seconds / 60)} more minutes.` : ""}` }),
    ),
    el(
      "div",
      { class: "card pad" },
      el("span", { class: "card-t", text: "Session token" }),
      el("div", { class: "copyfield h34" }, el("span", { class: "t", text: P.token_preview || "…" }), btn({ class: "btn xs", onclick: rotateToken }, "Rotate")),
      el("span", { class: "muted t-sm pretty", text: `Handed to this page once by the printed URL and sent back as a ${P.token_header || "Semlith-Token"} header on every /api route. Never in a cookie.` }),
    ),
    el(
      "div",
      { class: "card" },
      el("div", { class: "card-h" }, el("span", { class: "card-t grow", text: "Start at login" }), toggle(!!login.installed, login.installed ? "installed" : "off", setLogin, { cls: "t125" })),
      [
        ["MECHANISM", login.mechanism || a.service?.status?.mechanism || "—"],
        ["DEFINITION", login.path ? tilde(login.path) : a.service?.status?.path ? tilde(a.service.status.path) : "not installed"],
        ["LAST STARTED", login.last_start ? `${ago(login.last_start)} · pid ${about.pid}` : a.service?.last_started ? ago(a.service.last_started) : "—"],
      ].map(([k, v]) => el("div", { class: "kv sm" }, el("span", { class: "k", text: k }), el("span", { class: "v", text: v }))),
    ),
  ];
}

async function revealKey() {
  if (seUi.reveal) {
    seUi.reveal = null;
    return repaint();
  }
  const out = await act(() => post("/api/agents/reveal", {}));
  if (out) {
    seUi.reveal = out.key;
    repaint();
  }
}

async function rotateKey() {
  const ok = await ask({ title: "Rotate the agent key?", body: "Registered clients keep working: they read the key from its file. Anything you pasted an HTTP stanza into needs the new key; the old one is honoured for fifteen minutes.", ok: "Rotate" });
  if (!ok) return;
  const out = await act(() => post("/api/key", {}), "Agent key rotated");
  if (out) {
    seUi.reveal = null;
    await loadMany(["agents", "privacy"], true);
    repaint();
  }
}

async function rotateToken() {
  const ok = await ask({ title: "Rotate the session token?", body: "This tab moves to the new token straight away. Any other open tab of the portal has to be reopened from the URL `semlith start` prints.", ok: "Rotate" });
  if (!ok) return;
  const out = await act(() => post("/api/rotate", {}), "Session token rotated · this tab re-authenticated");
  if (out && out.token) {
    session.set(out.token);
    await load("privacy", true);
    repaint();
  }
}

async function setLogin(on) {
  const ok = on ? true : await ask({ title: "Stop starting at login?", body: "The login service is removed. After a reboot, start semlith yourself with `semlith start` — agents cannot reach it until you do.", ok: "Remove it", danger: true });
  if (!ok) return;
  const out = await act(() => post("/api/login-item", { on }), on ? "Installed as a login service" : "Removed the login service");
  if (out) await load("about", true), repaint();
}

// Settings › Cloud. Not signed in: what the cloud adds and the two commands,
// no price. Signed in: per org its header, whether the host answered, a card
// per remote store, ledger sync per local store, and what leaves the machine.
const cloudUi = { status: null, loading: false, report: null, kind: "savings", format: "md", session: "", dirs: {} };

async function loadCloudStatus(force) {
  if (cloudUi.loading || (cloudUi.status && !force)) return;
  cloudUi.loading = true;
  repaint();
  try {
    cloudUi.status = await api("/api/cloud/status");
  } catch (e) {
    cloudUi.status = { orgs: [], error: e.message };
  }
  cloudUi.loading = false;
  repaint();
}

function lagWord(s) {
  if (s === null || s === undefined) return "no lag known";
  if (s < 120) return `${s} s behind`;
  if (s < 7200) return `${Math.round(s / 60)} min behind`;
  return `${Math.round(s / 3600)} h behind`;
}

function cloudFact(C) {
  if (!C || !C.signed_in) return ["Cloud", "not signed in", "no cloud call and no connection, until semlith cloud login"];
  const sync = (C.sync?.on || []).length;
  return ["Cloud", (C.orgs || []).map((o) => o.org).join(", "), `${(C.orgs || []).map((o) => o.host_name).join(", ")} only · ledger sync on ${plural(sync, "store")} · searches naming a remote store go there`];
}

function syncLine() {
  const C = data.cloud;
  if (!C || !C.signed_in) return null;
  const S = C.sync || {};
  const on = (S.on || []).length;
  const last = S.last_sent ? `last sent ${ago(S.last_sent)} (${plural(S.last_rows || 0, "row")} from ${S.last_store})` : on ? "nothing sent yet" : "nothing is sent";
  return el("div", { class: "muted t-sm", text: `syncing ${on} of ${plural(S.stores || 0, "store")} · ${last}${S.last_error ? ` · last try: ${S.last_error}` : ""}` });
}

function seCloud() {
  const C = data.cloud || {};
  if (!C.signed_in) return seCloudOff();
  if (!cloudUi.status && !cloudUi.loading) setTimeout(() => loadCloudStatus(), 0);
  return [...(C.orgs || []).map((o) => cloudOrgCard(o, C)), cloudSyncCard(C), cloudLeavesCard(C), cloudSendCard(C)];
}

function seCloudOff() {
  return el(
    "div",
    { class: "card" },
    el("div", { class: "card-h" }, el("span", { class: "card-t grow", text: "Semlith Cloud" }), pill("not connected", "grey", { dot: false })),
    el("div", { class: "card-b line-row muted t13 pretty", text: "An organisation's stores, hosted: every repository and document set indexed on push, served over MCP to any agent, with a pull-request impact check and a team ledger. This binary works without it and contacts nothing until you sign in." }),
    el(
      "div",
      { class: "auto-fit m200 gap0" },
      [
        ["One URL for cloud agents", "Cloud sessions and CI can't reach a laptop; they can reach an org's stores."],
        ["Its stores beside yours", "Connected, they answer with the local ones, in one ranked list, each row badged."],
        ["A pull-request check", "States what the graph proves, with no model and no guess."],
      ].map(([t, d]) => el("div", { class: "point" }, el("span", { class: "t-b t13", text: t }), el("span", { class: "muted t-xs", text: d }))),
    ),
    el(
      "div",
      { class: "card-b" },
      [
        ["semlith cloud login <org>", "Approve a code in the browser. The org token is kept in ~/.semlith/cloud.json and sent to that host only."],
        ["semlith cloud connect <org>", "Adds the org's stores beside the local ones, with a remote badge. Nothing in them is written from here."],
      ].map(([c, note]) => el("div", { class: "col gap4" }, copyField(c), el("span", { class: "muted t-xs", text: note }))),
    ),
  );
}

function cloudOrgCard(o, C) {
  const st = (cloudUi.status?.orgs || []).find((x) => x.org === o.org && x.host === o.host);
  const reach = st ? st.reach : "checking";
  const tone = { connected: "green", unreachable: "amber", refused: "red" }[reach] || "grey";
  const org = st?.status?.org || {};
  const remote = (C.remote || []).filter((r) => r.org === o.org && r.host === o.host);
  const listed = st?.status?.stores || [];
  return el(
    "div",
    { class: "card" },
    el(
      "div",
      { class: "card-h" },
      el("span", { class: "card-t grow", text: org.name ? `${org.name} · ${o.org}` : o.org }),
      pill(org.plan || o.plan || "plan not known yet", "blue", { dot: false }),
      el("span", { class: "mono t-xs muted", "data-tip": "The token's prefix; the token itself is never shown", text: `${o.prefix}…` }),
      btn({ class: "btn xs", onclick: () => disconnectCloud(o) }, "Disconnect"),
    ),
    el(
      "div",
      { class: "card-b" },
      el(
        "div",
        { class: "row gap8" },
        pill(reach, tone, { pulse: reach === "checking" }),
        el("span", { class: "muted t-sm grow pretty", text: st ? `${st.why}${st.version ? ` · cloud ${st.version}` : ""}` : `Asking ${o.host_name}…` }),
        btn({ class: "btn xs", disabled: cloudUi.loading ? true : null, onclick: () => loadCloudStatus(true) }, "Check again"),
      ),
    ),
    remote.length
      ? remote.map((r) => remoteStoreBlock(r, listed.find((x) => x.name === r.store), reach === "connected" ? o.org : null))
      : el("div", { class: "card-b col gap6" }, el("span", { class: "muted t-sm", text: "No store of this org is connected on this machine." }), el("div", { class: "row" }, btn({ class: "btn sm", onclick: () => connectCloud(o) }, "Connect its stores"))),
  );
}

function remoteStoreBlock(r, s, answeredBy) {
  const freshDot = (state) => ({ fresh: "green", indexing: "amber", queued: "amber", paused: "", error: "red" })[state] || "";
  const input = el("input", {
    value: cloudUi.dirs[r.name] || "",
    placeholder: "Folder to push, e.g. ~/work/infra",
    "aria-label": `Folder to push to ${r.name}`,
    "data-keep": `push-${r.name}`,
    oninput: (e) => (cloudUi.dirs[r.name] = e.target.value),
  });
  return el(
    "div",
    { class: "card-b col gap6" },
    el("div", { class: "row gap8" }, el("span", { class: "t-m t13", text: r.name }), pill(r.badge, "blue", { dot: false, sm: true }), el("span", { class: "muted t-xs grow", text: s ? `${s.state} · ${n(s.files || 0)} files · ${n(s.chunks || 0)} chunks` : answeredBy ? `${answeredBy} no longer offers this store: deleted, renamed, or out of this token's reach` : "not in the host's answer yet" })),
    (s?.sources || []).map((src) =>
      el(
        "div",
        { class: "dl-row" },
        el("span", { class: "row gap6" }, el("span", { class: `dot ${freshDot(src.state)}`, "data-tip": src.state }), el("span", { class: "t-sm", text: src.label })),
        el("span", { class: "muted t-xs mono", text: `${src.kind} · ${src.revision || "—"} · ${lagWord(src.behind_seconds)}` }),
      ),
    ),
    el("div", { class: "row gap6" }, el("div", { class: "box grow" }, input), btn({ class: "btn xs", onclick: () => pushCloud(r) }, "Push")),
  );
}

function cloudSyncCard(C) {
  const orgs = C.orgs || [];
  const on = new Map((C.sync?.on || []).map((x) => [x.store, x]));
  return el(
    "div",
    { class: "card" },
    el("div", { class: "card-h" }, el("span", { class: "card-t grow", text: "Ledger sync" }), meta("off until you turn it on, per store")),
    el("div", { class: "card-b muted t-sm pretty", text: "A synced store sends each retrieval's time, client, session, tool and token count to the org's ledger, once a minute while the daemon runs. Never the query text, never what was read." }),
    liveStores().map((s) =>
      el(
        "div",
        { class: "dl-row" },
        el("span", { class: "col" }, el("span", { class: "t-m t-sm", text: s.name }), el("span", { class: "muted t-xs", text: on.has(s.name) ? `to ${on.get(s.name).org} since ${clock(on.get(s.name).since)}` : "stays on this machine" })),
        toggle(on.has(s.name), on.has(s.name) ? "syncing" : "off", (v) => setCloudSync(s.name, v, orgs[0] && orgs[0].org)),
      ),
    ),
    el("div", { class: "card-foot sans" }, syncLine()),
  );
}

function cloudLeavesCard(C) {
  const hosts = (C.orgs || []).map((o) => o.host_name).join(", ");
  return el(
    "div",
    { class: "card" },
    el("div", { class: "card-h" }, el("span", { class: "card-t grow", text: "What leaves this machine" }), meta(`to ${hosts} and nowhere else`)),
    [
      ["A search or tool naming a remote store", "the query and its filters, when you or an agent asks; a search naming no store asks the connected ones too"],
      ["Ledger sync", "time, client, session, tool and token count of each retrieval from a synced store; never the text"],
      ["Push", "the files of the folder you name, after semlith's own refusals: secrets, files over 1 MB, vendored trees, .gitignore"],
      ["Report and replay", "a report request, and the one transcript you pick, only when you press the button"],
      ["The token", "in the Authorization header to its own host; its prefix is all this page shows"],
    ].map(([k, v]) => el("div", { class: "kv sm" }, el("span", { class: "k", text: k }), el("span", { class: "v", text: v }))),
  );
}

function cloudSendCard(C) {
  const org = (C.orgs || [])[0];
  if (!org) return null;
  const sessions = data.replay?.sessions || [];
  return el(
    "div",
    { class: "card" },
    el("div", { class: "card-h" }, el("span", { class: "card-t grow", text: "Reports and replay" }), meta(org.org)),
    el(
      "div",
      { class: "card-b" },
      el(
        "div",
        { class: "row gap8" },
      dropdown({ label: "Report", value: cloudUi.kind, options: ["savings", "audit", "brief", "health", "gaps"].map((k) => [k, k]), onChange: (v) => ((cloudUi.kind = v), repaint()) }),
      dropdown({ label: "Format", value: cloudUi.format, options: ["md", "csv", "json", "html"].map((k) => [k, k]), onChange: (v) => ((cloudUi.format = v), repaint()) }),
      btn({ class: "btn sm", onclick: () => fetchCloudReport(org) }, "Fetch report"),
      ),
    ),
    cloudUi.report ? el("pre", { class: "code wrap sm", text: cloudUi.report }) : null,
    el(
      "div",
      { class: "card-b" },
      el(
        "div",
        { class: "row gap8" },
      sessions.length
        ? dropdown({ label: "Session", value: cloudUi.session || sessions[0].id, options: sessions.map((x) => [x.id, `${x.project} · ${plural(x.answers, "answer")}`]), onChange: (v) => ((cloudUi.session = v), repaint()) })
        : el("span", { class: "muted t-sm grow", text: data.replay?.enabled === false ? "Session replay is off on the Privacy page, so no transcript is read." : "No transcript with a semlith call in it yet." }),
      sessions.length ? btn({ class: "btn sm", onclick: () => sendReplay(org, cloudUi.session || sessions[0].id) }, "Send this session") : null,
      ),
    ),
  );
}

async function connectCloud(o) {
  const out = await act(() => post("/api/cloud/connect", { org: o.org }), (x) => `Connected ${plural((x && x.connected || []).length, "store")} of ${o.org}`);
  if (out) await loadMany(["cloud", "stores", "about"], true), loadCloudStatus(true);
}

async function disconnectCloud(o) {
  const ok = await ask({ title: `Disconnect ${o.org}?`, body: `Its remote stores leave this machine and its token is forgotten here. Nothing in the cloud changes; sign in again with semlith cloud login ${o.org}.`, ok: "Disconnect", danger: true });
  if (!ok) return;
  const out = await act(() => post("/api/cloud/disconnect", { org: o.org }), `Disconnected ${o.org}`);
  if (out) (cloudUi.status = null), await loadMany(["cloud", "stores", "about"], true), repaint();
}

async function setCloudSync(name, on, org) {
  const out = await act(() => post("/api/cloud/sync", { store: name, on, org }), on ? `${name} syncs its ledger rows` : `${name} stopped syncing`);
  if (out) await load("cloud", true), repaint();
}

async function pushCloud(r) {
  const dir = (cloudUi.dirs[r.name] || "").trim();
  if (!dir) return toast("Name the folder to push", true);
  const done = (x) => `${plural(x.files, "file")} in the manifest · ${x.sent} sent · ${x.removed} removed · job ${x.job}${x.position != null ? `, ${x.position} ahead of it` : ""}`;
  try {
    toast(done(await post("/api/cloud/push", { store: r.name, dir })));
  } catch (e) {
    // The cloud refuses a push that would remove uploads the folder lacks,
    // naming them; removing them is a separate, confirmed choice.
    if (!/--prune/.test(e.message)) return toast(e.message, true);
    const ok = await ask({ title: "Remove these files?", body: e.message.replace(/,? or push again with --prune to remove them\.?/, "."), ok: "Remove and push" });
    if (ok) await act(() => post("/api/cloud/push", { store: r.name, dir, prune: true }), done);
  }
}

async function fetchCloudReport(o) {
  const out = await act(() => post("/api/cloud/report", { org: o.org, kind: cloudUi.kind, format: cloudUi.format }));
  if (out) (cloudUi.report = out.text), repaint();
}

async function sendReplay(o, id) {
  const ok = await ask({ title: "Send this session?", body: `Every semlith call in ${id}, with what was asked and what the agent did next, goes to ${o.org}'s ledger. Nothing else is sent.`, ok: "Send" });
  if (!ok) return;
  await act(() => post("/api/cloud/replay", { org: o.org, session: id }), (x) => `Sent ${plural(x.sent, "answer")}; ${o.org} kept ${x.accepted}`);
}

function seAbout() {
  const a = data.about || {};
  const langs = data.languages?.languages || [];
  const withGraph = new Set(a.graph_languages || []);
  const prices = data.prices || {};
  const up = seUi.update;
  return [
    el(
      "div",
      { class: "split s-13-1 stretch" },
      el(
        "div",
        { class: "card" },
        [
          ["VERSION", `${a.version} · store format ${a.format_version}`],
          ["BINARY", `${shortPath(a.binary, 44)} · ${bytes(a.binary_bytes)} · ${a.target}`],
          ["BOUND TO", a.bind],
          ["STORE HOME", shortPath(a.store_home, 44)],
          ["SOURCE", `${a.license} · free and complete`],
          ["UPTIME", `${spellTook((a.uptime || 0) * 1000)} · pid ${a.pid}`],
        ].map(([k, v]) => el("div", { class: "kv" }, el("span", { class: "k", text: k }), el("span", { class: "v", text: v }))),
      ),
      el(
        "div",
        { class: "stack" },
        el(
          "div",
          { class: "card pad" },
          el("span", { class: "card-t", text: "Updates" }),
          el("span", { class: "muted t-sm pretty", text: up ? (up.error ? up.error : up.available ? `${up.latest} is out; this is ${up.installed}.${up.blocked ? ` It cannot be installed from here: ${up.blocked}` : ""}` : `${up.installed} is the latest release. Checked just now — that one request is the only time semlith asks.`) : "semlith never checks on its own. Press the button and it asks github.com once." }),
          el(
            "div",
            { class: "row" },
            btn({ class: "btn sm", onclick: checkUpdate }, up ? "Check again" : "Check for updates"),
            up && up.available && !up.blocked ? btn({ class: "btn sm primary", onclick: installUpdate }, `Install ${up.latest}`) : null,
          ),
        ),
        el(
          "div",
          { class: "card pad" },
          el("span", { class: "card-t", text: "Prices" }),
          el("span", { class: "muted t-sm pretty", text: `${n(prices.models)} models priced, from ${prices.source || "the built-in table"}${prices.fetched ? ` (${prices.fetched})` : ""}. Savings in Reports and the Ledger are priced from this table.` }),
          btn({ class: "btn sm", onclick: updatePrices }, "Update prices"),
        ),
        el("div", { class: "card pad dashed" }, el("span", { class: "card-t", text: "First-run screen" }), el("span", { class: "muted t-sm", text: "See the welcome and the machine checks again. Nothing is deleted." }), btn({ class: "btn sm dark", onclick: () => go("welcome") }, "Open the first-run screen")),
      ),
    ),
    el(
      "div",
      { class: "card" },
      el("div", { class: "card-h" }, el("span", { class: "card-t", text: plural(langs.length, "language") }), el("span", { class: "muted t-sm grow", text: "Search filters and the code graph read the same table, so the two cannot disagree." })),
      el(
        "div",
        { class: "auto-fill m200" },
        langs.map((l) => el("div", { class: "lang-cell", "data-tip": withGraph.has(l.name) ? "search and graph" : "search only — no grammar for edges" }, el("span", { class: withGraph.has(l.name) ? "ok" : "muted", text: withGraph.has(l.name) ? "✓" : "·" }), el("span", { class: "n", text: l.name }), el("span", { class: "spacer" }), el("span", { class: "e", text: [...(l.extensions || []).map((e) => `.${e}`), ...(l.filenames || [])].slice(0, 3).join(" ") }))),
      ),
    ),
  ];
}

async function checkUpdate() {
  try {
    seUi.update = await post("/api/upgrade", { action: "check" });
  } catch (e) {
    seUi.update = { error: e.message };
  }
  repaint();
}

async function installUpdate() {
  const up = seUi.update;
  const ok = await ask({ title: `Install ${up.latest}?`, body: "Downloads the release from github.com and replaces this binary. The running daemon keeps the old one until you restart it.", ok: "Install" });
  if (!ok) return;
  const out = await act(() => post("/api/upgrade", { action: "apply", version: up.latest }), (o) => o.restart || "Installed — restart semlith to use it");
  if (out) {
    seUi.update = { ...up, available: false, installed: up.latest };
    repaint();
  }
}

// --------------------------------------------------------------------- boot

async function boot() {
  let saved = null;
  try {
    saved = localStorage.getItem("semlith-theme");
  } catch (_) {
    /* private window */
  }
  state.theme = ["light", "dark", "system"].includes(saved) ? saved : "system";
  applyTheme();
  window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => state.theme === "system" && applyTheme());
  tip.wire();

  // The boot animation covers exactly the first fetches, and goes the frame
  // they answer.
  const done = openLoader(true);
  // The change counters' baseline first, so a change landing while the page
  // loads is one the first poll sees move.
  await pollChanges();
  // One dropped connection is not a daemon that is down: the first fetches
  // are asked again, with a growing pause, for about ten seconds before the
  // page says it could not reach it.
  let first = await loadMany(["stores", "about", "runs", "refused"]);
  for (const wait of [400, 800, 1600, 3000, 4000]) {
    if (data.stores) break;
    await new Promise((r) => setTimeout(r, wait));
    first = await loadMany(["stores", "about", "runs", "refused"], true);
  }
  done();
  const failed = first.find((x) => x && x.__error);
  if (failed && !data.stores) {
    fill(
      document.getElementById("root"),
      el(
        "div",
        { id: "app" },
        el("header", { class: "top wide-pad" }, logo(), el("span", { class: "wordmark", text: "Semlith" })),
        el("div", { class: "welcome-body" }, el("div", { class: "welcome-card max560" }, el("div", { class: "welcome-h", text: "The portal could not reach the daemon" }), el("div", { class: "welcome-lead", text: `${failed.__error.message} — trying again every few seconds.` }), btn({ class: "btn primary", onclick: () => location.reload() }, "Try again"))),
      ),
    );
    // It keeps asking, and loads the portal the moment the daemon answers,
    // so a daemon that was restarting needs no reload by hand; a move to
    // another page asks at once.
    const retry = async () => {
      try {
        await api("/api/stores");
        location.reload();
      } catch (_) {
        /* still down */
      }
    };
    setInterval(retry, 3000);
    window.addEventListener("hashchange", retry);
    return;
  }
  render();
  // Details the shell states on every page, filled in when they arrive.
  for (const key of ["agents", "ledger", "privacy"]) load(key).then(paintChrome).catch(() => {});
  startLive();
  window.addEventListener("hashchange", render);
  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && state.tier === "s" && state.navOpen && shell.nav) {
      setNav(false);
      return;
    }
    if (e.key !== "/" || e.metaKey || e.ctrlKey || e.altKey || (e.target && e.target.matches && e.target.matches("input, textarea, select, [contenteditable]"))) return;
    if (state.screen !== "app") return;
    e.preventDefault();
    if (state.route.page === "search") shell.main.querySelector('[data-keep="sr-q"]')?.focus();
    else go("search");
  });
}

boot();
