import assert from "node:assert/strict";
import test from "node:test";
import { insertLine, linkify, mergeTimelines, parse, parseMerged, playerFile, removeLine, sessions, timeline, toHtml } from "./notes.js";
import { pages } from "./vault.js";

const md = `---
session: 14
date: 2026-10-04
---
# Session 14 - 2026-10-04

- 20:01 Arrived in Phandalin
- 20:15 @[[Mirela]] the innkeeper, shifty
- 20:20 Met [[Baron Vex|the Baron]] at the gate
- 20:16 #+2 healing potions & 40gp
- 20:30 !Find the <missing> caravan
- 20:41 ?Why did the statue bleed
- 20:50 "You'll regret this" - Baron
- 21:02 Fought goblins
## Afterwards
We camped by the river.
- Bought rope
---
- 21:10 @
`;

test("groups notes by prefix, keeps hand-written lines, and escapes HTML", () => {
  const session = parse(md);
  assert.equal(session.title, "Session 14 - 2026-10-04");
  assert.deepEqual(session.groups.event, [
    "Arrived in Phandalin",
    "Met [[Baron Vex|the Baron]] at the gate",
    "Fought goblins",
    "We camped by the river.",
    "Bought rope",
  ]);
  assert.deepEqual(session.groups.npc, ["[[Mirela]] the innkeeper, shifty"]); // bare "@" dropped
  assert.deepEqual(session.groups.quote, [`"You'll regret this" - Baron`]);

  // The Journal recap: links become plain labels by default.
  const html = toHtml(session);
  assert.ok(html.startsWith("<h2>Session 14 - 2026-10-04</h2><h3>What happened</h3>"));
  assert.ok(html.includes("<li>Met the Baron at the gate</li>"));
  assert.ok(html.includes("<li>Mirela the innkeeper, shifty</li>"));
  assert.ok(html.includes("<li>+2 healing potions &amp; 40gp</li>"));
  assert.ok(html.includes("<li>Find the &lt;missing&gt; caravan</li>"));
  assert.ok(html.includes("<li>&quot;You&#39;ll regret this&quot; - Baron</li>"));
  assert.ok(!html.includes("<h3>Mysteries</h3><ul></ul>"));
  assert.ok(!html.includes("[["));

  // In-app view: links rendered by the caller.
  const linked = toHtml(session, (target, label) => `<a data-target="${target}">${label}</a>`);
  assert.ok(linked.includes('<li><a data-target="Mirela">Mirela</a> the innkeeper, shifty</li>'));
  assert.ok(linked.includes('<a data-target="Baron Vex">the Baron</a>'));
  assert.equal(toHtml(parse("# Empty\n")), "<h2>Empty</h2>");
});

test("timeline keeps every note in file order with time and kind", () => {
  const items = timeline(md);
  assert.deepEqual(items.slice(0, 4), [
    { time: "20:01", kind: "event", text: "Arrived in Phandalin", line: 6 },
    { time: "20:15", kind: "npc", text: "[[Mirela]] the innkeeper, shifty", line: 7 },
    { time: "20:20", kind: "event", text: "Met [[Baron Vex|the Baron]] at the gate", line: 8 },
    { time: "20:16", kind: "loot", text: "+2 healing potions & 40gp", line: 9 }, // file order, not sorted
  ]);
  assert.deepEqual(items.slice(-3), [
    { time: "21:02", kind: "event", text: "Fought goblins", line: 13 },
    { time: "", kind: "event", text: "We camped by the river.", line: 15 },
    { time: "", kind: "event", text: "Bought rope", line: 16 }, // no heading, rule or bare "@"
  ]);
  const { groups } = parse(md);
  for (const [kind, texts] of Object.entries(groups)) {
    assert.deepEqual(items.filter((i) => i.kind === kind).map((i) => i.text), texts);
  }
  assert.deepEqual(timeline("# Empty\n"), []);
});

test("links with apostrophes and ampersands survive escaping", () => {
  const a = (target, label) => `<a data-target="${target}">${label}</a>`;
  assert.equal(
    linkify("East along the [[Old King's Road]] & [[Salt & Iron|the guild]] <b>", a),
    `East along the <a data-target="Old King's Road">Old King&#39;s Road</a> &amp; <a data-target="Salt & Iron">the guild</a> &lt;b&gt;`,
  );
  assert.equal(linkify("[[Mirela#Secrets|she]] lied", a), '<a data-target="Mirela">she</a> lied');
  assert.equal(toHtml(parse("- 19:40 Took the [[Old King's Road]]")), "<h3>What happened</h3><ul><li>Took the Old King&#39;s Road</li></ul>");
});

test("a note's line can be removed and put back", () => {
  const lines = md.split("\n");
  for (const item of timeline(md)) assert.ok(lines[item.line].includes(item.text.slice(0, 10))); // line indexes point at the note
  assert.equal(timeline("# T\n- 20:00 a\r\n- 20:01 b")[1].line, 2); // no properties
  const { line } = timeline(md)[1];
  const without = removeLine(md, line, lines[line]);
  assert.ok(!without.includes("Mirela"));
  assert.equal(without.split("\n").length, lines.length - 1);
  assert.equal(insertLine(without, line, lines[line]), md);
  assert.throws(() => removeLine(md, line, "- 20:15 something else")); // the page changed: nothing removed
  assert.equal(insertLine("a", 5, "b"), "a\nb");
});

test("sessions newest first, with title, date and note count", () => {
  assert.deepEqual(sessions([
    { path: "Sessions/Session 2.md", content: "---\ndate: '2026-10-04'\n---\n# Session 2 at [[Phandalin|town]]\n- 20:00 a\n- 20:01 b" },
    { path: "Sessions/Session 10.md", content: "" },
    { path: "NPCs/Mirela.md", content: "- not a session" },
  ]), [
    { path: "Sessions/Session 10.md", title: "Session 10", date: "", count: 0 },
    { path: "Sessions/Session 2.md", title: "Session 2 at town", date: "2026-10-04", count: 2 },
  ]);
});

const shared = [
  { path: "Sessions/Session 4/Sibling 5.md", content: playerFile("Sessions/Session 4", "Sibling 5", "2026-10-06") + "- 20:05 @[[Mirela]] lies\n- 23:50 #Gold\n- 00:10 Slept\n" },
  { path: "Sessions/Session 4/Arn.md", content: playerFile("Sessions/Session 4", "Arn", "2026-10-06") + "- 20:05 Arrived\nThe rain stopped.\n- 21:00 ![[map.png]]\n" },
  { path: "Sessions/Session 4.md", content: "# Session 4\n- 20:00 Before sharing\n" },
];
const session4 = () => pages(shared).find((p) => p.path === "Sessions/Session 4");

test("a shared session's players' notes merge into one timeline, by time then author", () => {
  const items = mergeTimelines(session4().parts);
  assert.deepEqual(items.map((n) => [n.time, n.author, n.text]), [
    ["20:00", "", "Before sharing"], // written before the folder: no author
    ["20:05", "Arn", "Arrived"],
    ["", "Arn", "The rain stopped."], // a line without a time stays after the note before it
    ["20:05", "Sibling 5", "[[Mirela]] lies"],
    ["21:00", "Arn", "![[map.png]]"],
    ["23:50", "Sibling 5", "Gold"],
    ["00:10", "Sibling 5", "Slept"], // past midnight
  ]);
  // Each note knows its file and line, for deleting your own.
  const lies = items.find((n) => n.text.endsWith("lies"));
  assert.equal(lies.path, "Sessions/Session 4/Sibling 5.md");
  assert.equal(shared[0].content.split("\n")[lies.line], "- 20:05 @[[Mirela]] lies");
  assert.deepEqual(mergeTimelines([]), []);
});

test("the Journal of a shared session names each note's author", () => {
  const { title, groups } = parseMerged(session4().parts, ["PCs/Sibling 5.md"]);
  assert.equal(title, "Session 4");
  assert.deepEqual(groups.npc, ["[[Mirela]] lies ([[PCs/Sibling 5|Sibling 5]])"]);
  assert.deepEqual(groups.event.slice(0, 3), ["Before sharing", "Arrived ([[Arn|Arn]])", "The rain stopped. ([[Arn|Arn]])"]);
  assert.ok(groups.event.includes("![[map.png]]"), "an image alone gets no author");
  assert.ok(toHtml({ title: "", groups }).includes("<li>Mirela lies (Sibling 5)</li>"));
});

test("the sessions list counts a shared session's notes from every player", () => {
  assert.deepEqual(sessions(pages(shared)), [{ path: "Sessions/Session 4", title: "Session 4", date: "2026-10-06", count: 7 }]);
  assert.equal(playerFile("Sessions/Session 12", "Arn", "2026-10-06"), '---\nsession: 12\ndate: 2026-10-06\nauthor: "[[Arn]]"\n---\n# Session 12 - 2026-10-06\n\n');
});
