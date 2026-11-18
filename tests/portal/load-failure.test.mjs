// A page whose first load answered with an error must say so, rather than
// draw its empty state. loadFailure() is the decision render() makes from what
// loadMany() returned: the error of a needed key that has no cached data.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const source = readFileSync(path.join(here, "../../src/portal/app.js"), "utf8");
const start = source.indexOf("function loadFailure(");
assert.notEqual(start, -1, "loadFailure is missing from app.js");
const closing = /\r?\n\}\r?\n/.exec(source.slice(start));
const loadFailureSource = source.slice(start, start + closing.index + closing[0].length);
const loadFailure = new Function(`${loadFailureSource}\nreturn loadFailure;`)();

test("a needed key that failed with no cached data reports its error", () => {
  const error = new Error("500 from /api/ledger");
  assert.equal(loadFailure({}, ["ledger"], [{ __error: error }]), error);
});

test("a key that failed but still has cached data is not an error for the page", () => {
  const data = { ledger: { entries: [] } };
  assert.equal(loadFailure(data, ["ledger"], [{ __error: new Error("500") }]), null);
});

test("answers that arrived are not errors", () => {
  const data = { stores: { stores: [] } };
  assert.equal(loadFailure(data, ["stores"], [data.stores]), null);
});
