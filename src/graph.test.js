import { test } from "node:test";
import assert from "node:assert/strict";
import { connections, layout, neighbourhood, pageImage } from "./graph.js";

const images = ["Attachments/Mirela.jpg", "Attachments/Phandalin.png", "Maps/Cragmaw.webp"];

test("a page's picture: the portrait, else the first image in the text that exists", () => {
  assert.equal(pageImage(`---\nportrait: "[[Attachments/Mirela.jpg]]"\n---\n![[Phandalin.png]]`, images), "Attachments/Mirela.jpg");
  assert.equal(pageImage(`---\nportrait: [[Mirela.jpg|200]]\n---\n`, images), "Attachments/Mirela.jpg");
  assert.equal(pageImage("# Phandalin\n![[gone.png]]\nA town.\n![[Phandalin.png|300]]", images), "Attachments/Phandalin.png");
  assert.equal(pageImage("![the cave](Maps/Cragmaw.webp)", images), "Maps/Cragmaw.webp");
  assert.equal(pageImage(`---\nportrait: [[gone.jpg]]\n---\n![[Phandalin.png]]`, images), "Attachments/Phandalin.png");
  assert.equal(pageImage("No pictures, just [[Mirela]].", images), null);
  // IMAGE_EMBED is a /g regex: asking twice must give the same answer.
  assert.equal(pageImage("![[Phandalin.png]]", images), pageImage("![[Phandalin.png]]", images));
});

const notes = [
  { path: "NPCs/Mirela.md", content: "Runs the inn in [[Phandalin]]. Owes [[Demus]]. [[Phandalin|the town]] again. ![[Mirela.jpg]]" },
  { path: "PCs/Demus.md", content: "Met [[Mirela]] and [[Mirela]]. Sworn to [[Lords' Alliance]]. Self: [[Demus]]." },
  { path: "Locations/Phandalin.md", content: "![[Phandalin.png]] A town. See [[Old Owl Well]]." },
  { path: "Factions/Lords' Alliance.md", content: "" },
  { path: "Quests/Find Gundren.md", content: "[[Mirela]] knows something." },
  { path: "Sessions/Session 1.md", content: "- 20:01 @[[Mirela]] at [[Phandalin]]" },
  { path: "Hermit.md", content: "[[Mirela]]" },
];

test("the map: people, places and factions, each link once and either way", () => {
  const { nodes, edges } = connections(notes, images);
  assert.deepEqual(nodes.map((n) => n.path), ["Factions/Lords' Alliance.md", "Locations/Phandalin.md", "NPCs/Mirela.md", "PCs/Demus.md"]);
  assert.deepEqual(nodes.map((n) => n.kind), ["faction", "location", "npc", "pc"]);
  assert.equal(nodes.find((n) => n.path === "NPCs/Mirela.md").image, "Attachments/Mirela.jpg");
  assert.equal(nodes.find((n) => n.path === "Factions/Lords' Alliance.md").image, null);
  // Mirela<->Demus is linked both ways but listed once; links to missing pages, sessions and quests are left out.
  assert.deepEqual(edges, [["NPCs/Mirela.md", "Locations/Phandalin.md"], ["NPCs/Mirela.md", "PCs/Demus.md"], ["PCs/Demus.md", "Factions/Lords' Alliance.md"]]);
});

test("private notes never link anything on the map", () => {
  const shared = [
    { path: "NPCs/Halia.md", content: "# Halia" },
    { path: "NPCs/Lorelei.md", content: "# Lorelei" },
  ];
  const secret = [
    { path: "Private/NPCs/Vex.md", content: "Vex knows [[Halia]] and [[Lorelei]]." },
    { path: "Private/Sessions/Session 4/Sibling 5.md", content: "- 20:01 @[[Halia]] met [[Lorelei]] at night" },
  ];
  const alone = connections(shared, images);
  assert.deepEqual(alone.nodes.map((n) => n.path), ["NPCs/Halia.md", "NPCs/Lorelei.md"]);
  assert.deepEqual(alone.edges, []);
  for (const file of secret) assert.deepEqual(connections([...shared, file], images), alone, file.path);
  assert.deepEqual(connections([...shared, ...secret], images), alone);
});

test("one link away", () => {
  const graph = connections(notes, images);
  const near = neighbourhood(graph, "PCs/Demus.md");
  assert.deepEqual(near.nodes.map((n) => n.path), ["Factions/Lords' Alliance.md", "NPCs/Mirela.md", "PCs/Demus.md"]);
  assert.deepEqual(near.edges, [["NPCs/Mirela.md", "PCs/Demus.md"], ["PCs/Demus.md", "Factions/Lords' Alliance.md"]]);
  assert.deepEqual(neighbourhood(graph, "Quests/Find Gundren.md"), { nodes: [], edges: [] });
});

test("the layout is the same every time, spreads nodes apart and keeps linked ones closer", () => {
  const nodes = Array.from({ length: 30 }, (_, i) => ({ path: `NPCs/${i}.md` }));
  const edges = nodes.slice(1).map((n, i) => [nodes[i % 5].path, n.path]); // five hubs
  const a = layout(nodes, edges);
  assert.deepEqual(a, layout(nodes, edges));
  const pts = [...a.values()];
  assert.ok(pts.every((p) => Number.isFinite(p.x) && Number.isFinite(p.y)));
  const dist = (p, q) => Math.hypot(p.x - q.x, p.y - q.y);
  let closest = Infinity;
  for (let i = 0; i < pts.length; i++) for (let j = i + 1; j < pts.length; j++) closest = Math.min(closest, dist(pts[i], pts[j]));
  assert.ok(closest > 50, `nodes overlap: ${closest}`); // two radii of 22, plus a gap
  const avg = (pairs) => pairs.reduce((s, [p, q]) => s + dist(a.get(p), a.get(q)), 0) / pairs.length;
  const linked = new Set(edges.map((e) => e.join()));
  const unlinked = nodes.flatMap((p, i) => nodes.slice(i + 1).map((q) => [p.path, q.path])).filter((e) => !linked.has(e.join()) && !linked.has([...e].reverse().join()));
  assert.ok(avg(edges) < avg(unlinked));
});
