// The Search page's RESULTS − and + must search again when there is a query,
// as LEAN TO and BUDGET do; otherwise the dial shows a count the page does not.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const source = readFileSync(path.join(here, "../../src/portal/app.js"), "utf8");

// The onclick expression of the stepper button with this label, cut out of
// app.js: from after "onclick: " to the comma or closing brace that ends it.
function onclickOf(label) {
  const line = source.split(/\r?\n/).find((l) => l.includes(`"aria-label": "${label}"`));
  assert.ok(line, `no button labelled "${label}" found`);
  const start = line.indexOf("onclick: ") + "onclick: ".length;
  let depth = 0;
  let end = start;
  for (; end < line.length; end++) {
    const c = line[end];
    if ("([{".includes(c)) depth++;
    else if (")]}".includes(c)) {
      if (depth === 0) break;
      depth--;
    } else if (c === "," && depth === 0) break;
  }
  return line.slice(start, end);
}

// Runs the button's real handler against a stub search state and records how
// often the page repaints and how often it asks for a search.
function press(label, query, k) {
  const sr = { query, k, prefer: "either", budget: 1000 };
  const calls = { repaints: 0, searches: 0 };
  const handler = new Function("sr", "repaint", "runSearch", "Math", `return (${onclickOf(label)});`)(
    sr,
    () => calls.repaints++,
    () => calls.searches++,
    Math,
  );
  handler();
  return { sr, calls };
}

for (const [label, from, to] of [
  ["Fewer results", 8, 7],
  ["More results", 8, 9],
]) {
  test(`the RESULTS stepper's "${label}" button re-runs the search`, () => {
    const { sr, calls } = press(label, "rust", from);
    assert.equal(sr.k, to, `the "${label}" button should move the count to ${to}`);
    assert.equal(calls.repaints, 1, `the "${label}" button should repaint once`);
    assert.equal(calls.searches, 1, `the RESULTS stepper "${label}" button only repaints; it does not run the search again`);
  });

  test(`the RESULTS stepper's "${label}" button does not search without a query`, () => {
    const { calls } = press(label, "", from);
    assert.equal(calls.searches, 0, `the "${label}" button searched with no query`);
  });
}
