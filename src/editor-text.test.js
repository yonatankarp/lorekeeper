import assert from "node:assert/strict";
import test from "node:test";
import { cycleHeading, diff, linkTarget, nameQuery, togglePrefix } from "./editor-text.js";

test("link targets drop the heading and alias", () => {
  assert.equal(linkTarget("Mirela"), "Mirela");
  assert.equal(linkTarget("Baron Vex|the Baron"), "Baron Vex");
  assert.equal(linkTarget(" Phandalin#Inn|town "), "Phandalin");
});

test("heading cycles none, H1, H2, H3, none", () => {
  assert.equal(cycleHeading("Title"), "# Title");
  assert.equal(cycleHeading("# Title"), "## Title");
  assert.equal(cycleHeading("## Title"), "### Title");
  assert.equal(cycleHeading("### Title"), "Title");
  assert.equal(cycleHeading("#### Title"), "Title");
  assert.equal(cycleHeading("#hashtag"), "# #hashtag");
});

test("list, task and quote prefixes toggle and replace each other", () => {
  assert.equal(togglePrefix("milk", "- "), "- milk");
  assert.equal(togglePrefix("- milk", "- "), "milk");
  assert.equal(togglePrefix("  * milk", "- "), "  milk");
  assert.equal(togglePrefix("- milk", "- [ ] "), "- [ ] milk");
  assert.equal(togglePrefix("- [x] milk", "- [ ] "), "milk");
  assert.equal(togglePrefix("- [ ] milk", "- "), "- milk");
  assert.equal(togglePrefix("said it", "> "), "> said it");
  assert.equal(togglePrefix("> said it", "> "), "said it");
  assert.equal(togglePrefix("", "- [ ] "), "- [ ] ");
});

test("name queries after @ or [[", () => {
  assert.deepEqual(nameQuery("- 20:15 @Mir"), { from: 9, link: false });
  assert.deepEqual(nameQuery("@Mir"), { from: 1, link: false });
  assert.deepEqual(nameQuery("met [[Baron V"), { from: 6, link: true });
  assert.deepEqual(nameQuery("see [["), { from: 6, link: true });
  assert.equal(nameQuery("mail a@b"), null);
  assert.equal(nameQuery("just @"), null);
  assert.equal(nameQuery("[[Done]] x"), null);
});

test("diff finds the one changed span", () => {
  assert.deepEqual(diff("abc", "abc"), { from: 3, to: 3, insert: "" });
  assert.deepEqual(diff("- a\n", "- a\n- b\n"), { from: 4, to: 4, insert: "- b\n" });
  assert.deepEqual(diff("hello world", "hello brave world"), { from: 6, to: 6, insert: "brave " });
  assert.deepEqual(diff("aaa", "aa"), { from: 2, to: 3, insert: "" });
  assert.deepEqual(diff("x", ""), { from: 0, to: 1, insert: "" });
});
