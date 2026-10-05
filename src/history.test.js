import assert from "node:assert/strict";
import test from "node:test";
import { navHistory, undoStack } from "./history.js";

test("back and forward skip gone pages and the page you're on", () => {
  const nav = navHistory();
  for (const p of [null, "A", "B", "B", "C"]) nav.visit(p); // re-opening B adds nothing
  const all = () => true;
  assert.equal(nav.find(-1, all, "C"), 2);
  assert.equal(nav.find(1, all, "C"), -1);
  assert.equal(nav.find(-1, (p) => p !== "B", "C"), 1); // B deleted: skipped
  assert.equal(nav.go(0), null); // back to Home
  assert.equal(nav.find(-1, all, null), -1);
  assert.equal(nav.find(1, all, null), 1);
  nav.visit("D"); // a new page drops the forward entries
  assert.equal(nav.find(1, all, "D"), -1);
  assert.equal(nav.entry(nav.find(-1, all, "D")), null);
  const twice = navHistory();
  for (const p of [null, "A", null]) twice.visit(p);
  assert.equal(twice.find(-1, (p) => p !== "A", null), -1); // only Home again, which is where you are
});

test("undo stack: undo, redo, a new action clears redo, cap, failures drop the action", async () => {
  const log = [];
  const act = (name, fail) => ({ label: name, undo: async () => { if (fail) throw "nope"; log.push(`-${name}`); }, redo: async () => log.push(`+${name}`) });
  const s = undoStack(2);
  assert.equal(await s.undo(), null);
  s.push(act("a"));
  s.push(act("b"));
  s.push(act("c")); // "a" falls off
  assert.equal((await s.undo()).label, "c");
  assert.equal((await s.redo()).label, "c");
  assert.equal((await s.undo()).label, "c");
  assert.equal((await s.undo()).label, "b");
  assert.equal(await s.undo(), null);
  s.push(act("d")); // clears redo
  assert.equal(await s.redo(), null);
  assert.deepEqual(log, ["-c", "+c", "-c", "-b"]);
  s.push(act("e", true));
  await assert.rejects(s.undo());
  assert.equal(s.top().label, "d"); // the failed one is gone
});

test("a renamed page keeps its place in history", () => {
  const nav = navHistory();
  for (const p of [null, "A", "B"]) nav.visit(p);
  nav.rename("A", "Z");
  assert.equal(nav.entry(nav.find(-1, (p) => p !== "A", "B")), "Z");
});
