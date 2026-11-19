// Counts on the portal read "1 file", "1 day" and "2 refunds", never "1 files"
// or "0 hours": every count goes through plural(), which pluralises on the
// number it prints.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const source = readFileSync(path.join(here, "../../src/portal/app.js"), "utf8");

// The one-line definitions of n and plural, taken from the page itself.
const line = (re) => {
  const m = re.exec(source);
  assert.ok(m, `app.js no longer has ${re}`);
  return m[0];
};
const defs = `${line(/^const n = .*$/m)}\n${line(/^const plural = .*$/m)}`;
const { n, plural } = new Function("NUM", `${defs}\nreturn { n, plural };`)(new Intl.NumberFormat("en-US"));

test("plural() pluralises on the count it prints", () => {
  assert.equal(plural(1, "file"), "1 file");
  assert.equal(plural(0, "file"), "0 files");
  assert.equal(plural(2, "refund"), "2 refunds");
  assert.equal(plural(1, "miss", "misses"), "1 miss");
  assert.equal(plural(3, "miss", "misses"), "3 misses");
  // 1.2 prints as "1", so it is one day, not "1 days".
  assert.equal(plural(1.2, "day"), "1 day");
  assert.equal(plural(1.6, "day"), "2 days");
  assert.equal(n(1.2), "1");
});

test("Reading time shows minutes under an hour, then hours, then days", () => {
  const expression = /const reading = (.*);$/m.exec(source);
  assert.ok(expression, "the Reading time KPI no longer has a reading expression");
  const read = (minutes) => new Function("plural", "minutes", `return ${expression[1]};`)(plural, minutes);
  assert.equal(read(20), "20 minutes");
  assert.equal(read(1), "1 minute");
  assert.equal(read(60), "1 hour");
  assert.equal(read(180), "3 hours");
  assert.equal(read(1500), "1 day");
  assert.equal(read(2160), "2 days");
});

test("no count in app.js is a raw template followed by a plural noun", () => {
  const nouns = [
    "files?", "chunks?", "days?", "hours?", "minutes?", "runs?", "stores?", "folders?", "models?",
    "refunds?", "miss(?:es)?", "items?", "queries", "query", "retrievals?", "paths?", "hits?",
    "lines?", "tools?", "clients?", "rows?", "projects?",
  ];
  const raw = new RegExp(String.raw`\$\{[^}]*\}\s*(?:${nouns.join("|")})\b(?!/)`, "g");
  const offenders = [];
  source.split(/\r?\n/).forEach((text, i) => {
    // A branch already guarded by `=== 1` only prints the plural for the rest.
    if (text.includes("=== 1")) return;
    for (const m of text.matchAll(raw)) offenders.push(`app.js:${i + 1}: ${m[0]}`);
  });
  assert.deepEqual(offenders, [], "route these through plural()");
});

test("percentages have no space before the sign", () => {
  const offenders = [];
  source.split(/\r?\n/).forEach((text, i) => {
    if (/[\d}]\s%["`]/.test(text)) offenders.push(`app.js:${i + 1}`);
  });
  assert.deepEqual(offenders, []);
});
