// The portal's copy() must not announce "Copied" when the browser refused the
// copy, and must put focus back on the button that was pressed.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const source = readFileSync(path.join(here, "../../src/portal/app.js"), "utf8");
const start = source.indexOf("async function copy(");
const closing = /\r?\n\}\r?\n/.exec(source.slice(start));
const copySource = source.slice(start, start + closing.index + closing[0].length);

test("a refused copy is announced as failed and focus returns to the button", async () => {
  const button = { focus() { document.activeElement = this; } };
  const shown = [];
  const document = {
    activeElement: button,
    body: { append() {} },
    execCommand: () => false,
  };
  // The fallback textarea takes focus when selected and drops it when removed,
  // as it does in a browser.
  const el = () => ({
    value: "",
    select() { document.activeElement = this; },
    remove() { document.activeElement = document.body; },
  });
  const toast = (message, bad) => shown.push({ message, bad });
  const navigator = { clipboard: { writeText: async () => { throw new Error("denied"); } } };

  const copy = new Function("el", "toast", "navigator", "document", `${copySource}\nreturn copy;`)(
    el, toast, navigator, document,
  );
  await copy("semlith index ~/x", "Copied");

  assert.equal(shown.length, 1);
  assert.equal(shown[0].bad, true, "a refused copy was announced as success");
  assert.match(shown[0].message, /copy it by hand/);
  assert.equal(document.activeElement, button, "focus did not return to the pressed button");
});
