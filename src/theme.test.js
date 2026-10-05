// The Dungeon tokens are written twice in each themed page (OS dark under System, and Settings forcing dark); CSS can't
// share one block between a media query and an attribute selector, so this keeps the copies from drifting apart.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const body = (css, selector) => {
  const start = css.indexOf("{", css.indexOf(selector)) + 1;
  let depth = 1, i = start;
  while (depth) depth += { "{": 1, "}": -1 }[css[i++]] ?? 0;
  return css.slice(start, i - 1).replace(/\s+/g, " ").trim();
};

for (const file of ["styles.css", "capture.html"]) {
  test(`${file}: both Dungeon copies match`, () => {
    const css = readFileSync(new URL(file, import.meta.url), "utf8");
    const media = body(css, ":root:not([data-theme])");
    assert.ok(media.includes("--fg:"));
    assert.equal(media, body(css, ':root[data-theme="dark"]'));
  });
}
