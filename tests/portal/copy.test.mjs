// The portal's copy() must not announce "Copied" when the browser refused the
// copy, must put focus back on the button that was pressed, and must only show
// text in the failure toast when the user can read and select it in time.
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

// Runs copy(text) with a clipboard and execCommand that both refuse the copy,
// and returns the toasts shown and the element that ended up focused.
async function refusedCopy(text) {
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
  await copy(text, "Copied");
  return { shown, focused: document.activeElement, button };
}

test("a refused copy is announced as failed and focus returns to the button", async () => {
  const { shown, focused, button } = await refusedCopy("semlith index ~/x");

  assert.equal(shown.length, 1);
  assert.equal(shown[0].bad, true, "a refused copy was announced as success");
  assert.match(shown[0].message, /copy it by hand/);
  assert.match(shown[0].message, /semlith index ~\/x/);
  assert.equal(focused, button, "focus did not return to the pressed button");
});

test("a refused copy of long or multi-line text does not put the text in the toast", async () => {
  const long = "/home/someone/projects/" + "a-very-long-directory-name/".repeat(8) + "file.rs";
  assert.ok(long.length > 120);
  const { shown, focused, button } = await refusedCopy(long);

  assert.equal(shown.length, 1);
  assert.equal(shown[0].bad, true, "a refused copy was announced as success");
  assert.equal(shown[0].message, "Copy failed, the browser blocked the clipboard");
  assert.ok(!shown[0].message.includes(long), "long text was put in the toast");
  assert.equal(focused, button, "focus did not return to the pressed button");

  const lines = await refusedCopy("first/path\nsecond/path");
  assert.equal(lines.shown[0].message, "Copy failed, the browser blocked the clipboard");
  assert.ok(!lines.shown[0].message.includes("first/path"), "multi-line text was put in the toast");
});
