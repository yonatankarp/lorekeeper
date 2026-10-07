import assert from "node:assert/strict";
import test from "node:test";
import { cheatSheetHtml, NOTE_PREFIXES, PRIVATE_PREFIX, readable } from "./cheatsheet.js";
import { escape, PREFIX, privateNote, timeline } from "./notes.js";

// Every character the parser might treat as a symbol: ASCII, typographic quotes and dashes, and the rest of the BMP.
const candidates = [...Array(0xffff - 0x20).keys()].map((i) => String.fromCharCode(i + 0x21)).filter((c) => !/[\s\p{Cs}]/u.test(c));

test("the cheat sheet lists every note symbol the parser files by, as the kind it files", () => {
  const listed = Object.fromEntries(NOTE_PREFIXES.flatMap(({ keys, kind }) => keys.map((k) => [k, kind])));
  const parsed = {};
  for (const c of candidates) {
    const kind = timeline(`- 21:43 ${c}x`)[0]?.kind ?? "event";
    if (kind !== "event") parsed[c] = kind;
  }
  assert.deepEqual(listed, parsed);
  assert.deepEqual(listed, PREFIX); // the editor colours lines by the same map
  const html = cheatSheetHtml();
  for (const k of Object.keys(listed)) assert.ok(html.includes(`<kbd>${escape(k)}</kbd>`), k);
});

test("the cheat sheet lists the private-note symbol, and it's the only one", () => {
  assert.deepEqual(candidates.filter((c) => privateNote(`${c}x`).private), [PRIVATE_PREFIX]);
  assert.ok(cheatSheetHtml().includes(`<kbd>${PRIVATE_PREFIX}</kbd>`));
});

test("shortcuts show as each platform writes them", () => {
  assert.equal(readable("CmdOrCtrl+Alt+KeyN", true), "⌘⌥N");
  assert.equal(readable("CmdOrCtrl+Alt+N", false), "Ctrl+Alt+N");
  assert.equal(readable("CmdOrCtrl+Backspace", true), "⌘⌫");
  assert.equal(readable("CmdOrCtrl+Backspace", false), "Ctrl+Backspace");
  assert.equal(readable("CmdOrCtrl+Slash", false), "Ctrl+/");
});

test("the cheat sheet shows the user's own global shortcuts, escaped", () => {
  const html = cheatSheetHtml({ settings: { quickNote: "CmdOrCtrl+Shift+Q", capture: "Alt+<b>", newSession: "" }, mac: false });
  assert.ok(html.includes("<kbd>Ctrl+Shift+Q</kbd>"), html);
  assert.ok(html.includes("<kbd>Alt+&lt;b&gt;</kbd>") && !html.includes("<b>"), html);
  assert.ok(html.includes("<th scope=\"row\">Off</th><td><strong>New session</strong>"), html);
  assert.ok(cheatSheetHtml({ mac: true }).includes("<kbd>⌘⌥N</kbd>")); // the default before settings load
});
