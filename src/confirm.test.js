import { test } from "node:test";
import assert from "node:assert/strict";
import { confirmClick } from "./confirm.js";

/** Just enough of a <button>: events, a label, data attributes and focus. */
function fakeButton(label) {
  const b = new EventTarget();
  Object.assign(b, { textContent: label, dataset: {}, focused: false, focus() { b.focused = true; } });
  return b;
}

test("Remove asks once, then acts on the second click", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const b = fakeButton("Remove"), said = [];
  let removed = 0;
  confirmClick(b, { armedLabel: "Remove Vex? Click again", prompt: "Click again to remove Vex.", act: () => removed++, say: (s) => said.push(s) });
  const click = () => b.dispatchEvent(new Event("click"));

  click();
  assert.equal(removed, 0, "the first click only asks");
  assert.equal(b.textContent, "Remove Vex? Click again");
  assert.equal(b.dataset.armed, "true", "styled as armed");
  assert.ok(b.focused, "focused, so moving away disarms it");
  assert.deepEqual(said, ["Click again to remove Vex."]);
  click();
  assert.equal(removed, 1);
  assert.equal(b.textContent, "Remove");
  assert.equal(b.dataset.armed, undefined);

  // After about 5 seconds it goes back, and the next click asks again.
  click();
  t.mock.timers.tick(4999);
  assert.equal(b.textContent, "Remove Vex? Click again");
  t.mock.timers.tick(1);
  assert.equal(b.textContent, "Remove");
  assert.equal(b.dataset.armed, undefined);
  assert.equal(said.at(-1), "", "the prompt goes too");
  click();
  assert.equal(removed, 1);

  // Moving focus away (Tab, or clicking elsewhere) goes back too.
  b.dispatchEvent(new Event("blur"));
  assert.equal(b.textContent, "Remove");
  click();
  assert.equal(removed, 1, "asked again after the blur");
  click();
  assert.equal(removed, 2);
});
