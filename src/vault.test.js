import assert from "node:assert/strict";
import test from "node:test";
import { backlinks, badName, buildTree, fillTemplate, folderFor, openQuests, party, questStatus, recentlyMentioned, resolve, search, sessionPaths, splitFrontmatter } from "./vault.js";

const notes = [
  { path: "Sessions/Session 2.md", content: "# Session 2\n- 20:15 @[[Mirela]] again\n- 20:30 went to [[Locations/Phandalin|town]]" },
  { path: "Sessions/Session 10.md", content: "# Session 10\n- 21:00 [[mirela#Secrets|she]] lied\n- 21:05 [[Nobody]] here" },
  { path: "NPCs/Mirela.md", content: "---\ntype: npc\nrace: elf\n---\n# Mirela\nInnkeeper." },
  { path: "Locations/Phandalin.md", content: "# Phandalin" },
  { path: "Archive/NPCs/Mirela.md", content: "old copy" },
];
const paths = notes.map((n) => n.path);

test("resolves links like Obsidian", () => {
  assert.equal(resolve("Mirela", paths), "NPCs/Mirela.md"); // shortest path wins
  assert.equal(resolve("mirela", paths), "NPCs/Mirela.md"); // case-insensitive
  assert.equal(resolve("Archive/NPCs/Mirela", paths), "Archive/NPCs/Mirela.md");
  assert.equal(resolve("Locations/Phandalin.md", paths), "Locations/Phandalin.md");
  assert.equal(resolve("Nobody", paths), null);
  assert.equal(resolve("ela", paths), null); // no partial names
  assert.equal(resolve("", paths), null);
});

test("backlinks find every linking line, including aliases and headings", () => {
  assert.deepEqual(backlinks("NPCs/Mirela.md", notes), [
    { path: "Sessions/Session 2.md", lines: ["- 20:15 @[[Mirela]] again"] },
    { path: "Sessions/Session 10.md", lines: ["- 21:00 [[mirela#Secrets|she]] lied"] },
  ]);
  assert.deepEqual(backlinks("Locations/Phandalin.md", notes).map((b) => b.path), ["Sessions/Session 2.md"]);
  assert.deepEqual(backlinks("Archive/NPCs/Mirela.md", notes), []);
});

test("tree sorts naturally and keeps empty folders", () => {
  const tree = buildTree(["Sessions", "NPCs", "PCs"], paths);
  assert.deepEqual(tree.dirs.map((d) => d.name), ["Archive", "Locations", "NPCs", "PCs", "Sessions"]);
  assert.deepEqual(tree.dirs[4].files, ["Sessions/Session 10.md", "Sessions/Session 2.md"]); // newest first
  assert.deepEqual(buildTree([], ["NPCs/Bob 10.md", "NPCs/Bob 2.md"]).dirs[0].files, ["NPCs/Bob 2.md", "NPCs/Bob 10.md"]);
  assert.equal(tree.dirs[0].dirs[0].path, "Archive/NPCs");
  assert.deepEqual(tree.dirs[3].files, []);
});

test("frontmatter, search, templates, names", () => {
  assert.deepEqual(splitFrontmatter(notes[2].content), {
    props: [["type", "npc"], ["race", "elf"]],
    body: "# Mirela\nInnkeeper.",
  });
  assert.deepEqual(splitFrontmatter("# No props"), { props: [], body: "# No props" });

  const hits = search("mirela", notes);
  assert.deepEqual(hits.map((h) => h.path).slice(0, 2), ["Archive/NPCs/Mirela.md", "NPCs/Mirela.md"]); // names first
  assert.equal(hits.find((h) => h.path === "Sessions/Session 2.md").snippet, "- 20:15 @[[Mirela]] again");
  assert.deepEqual(search("  ", notes), []);

  assert.equal(fillTemplate("# {{title}}\nMet {{date}}, {{title}}", "Bob", "2026-10-04"), "# Bob\nMet 2026-10-04, Bob");
  assert.equal(folderFor("NPC", ["NPCs", "PCs"]), "NPCs");
  assert.equal(folderFor("Monster", ["NPCs"]), "");
  assert.equal(badName("Mirela"), "");
  assert.ok(badName("a/b"));
  assert.ok(badName("[[x]]"));
  assert.ok(badName("  "));
});

test("quest status and the open quest list", () => {
  assert.equal(questStatus("---\ntype: quest\nstatus: done\n---\n# A"), "done");
  assert.equal(questStatus('---\nStatus: "Failed"\n---\n'), "failed");
  assert.equal(questStatus("---\nstatus:\n---\n"), "open"); // empty
  assert.equal(questStatus("# No properties"), "open");
  const quests = [
    { path: "Quests/Find the caravan.md", content: "---\nstatus: done\n---\n" },
    { path: "Quests/Rescue Sildar.md", content: "---\nstatus: Open\n---\n" },
    { path: "Quests/Quest 10.md", content: "# no status" },
    { path: "Quests/Quest 2.md", content: "---\nstatus: 'open'\n---\n" },
    { path: "Quests/Slay the dragon.md", content: "---\nstatus: failed\n---\n" },
    { path: "Quests/Later.md", content: "---\nstatus: on hold\n---\n" },
    { path: "NPCs/Mirela.md", content: "---\nstatus: open\n---\n" }, // not a quest page
  ];
  assert.deepEqual(openQuests(quests), ["Quests/Quest 2.md", "Quests/Quest 10.md", "Quests/Rescue Sildar.md"]);
  assert.deepEqual(openQuests([]), []);
});

test("party: PCs with their properties, sorted", () => {
  const pcs = [
    { path: "PCs/Pip.md", content: "---\nClass: rogue\nrace: 'halfling'\nlevel: 3\nplayer: Sam\n---\n# Pip" },
    { path: "PCs/Brakka.md", content: "# Brakka, no properties" },
    { path: "NPCs/Mirela.md", content: "---\nclass: bard\n---\n" },
  ];
  assert.deepEqual(party(pcs), [
    { path: "PCs/Brakka.md", class: "", race: "", level: "", player: "" },
    { path: "PCs/Pip.md", class: "rogue", race: "halfling", level: "3", player: "Sam" },
  ]);
  assert.deepEqual(party([]), []);
});

test("recently mentioned: pages linked from the last two sessions, most mentioned first", () => {
  const vault = [
    { path: "Sessions/Session 1.md", content: "- [[Old Town]] [[Old Town]] [[Old Town]]" }, // too old
    { path: "Sessions/Session 2.md", content: "- @[[Mirela]] at [[Phandalin]]\n- [[The Crows|crows]] [[Nobody]] [[Pip]]" },
    { path: "Sessions/Session 10.md", content: "- [[mirela]] lied, [[Crow Signet]]\n- [[Phandalin#Inn|inn]] [[Mirela]]" },
    { path: "NPCs/Mirela.md", content: "" },
    { path: "Locations/Phandalin.md", content: "" },
    { path: "Locations/Old Town.md", content: "" },
    { path: "Items/Crow Signet.md", content: "" },
    { path: "Factions/The Crows.md", content: "" },
    { path: "PCs/Pip.md", content: "" }, // the party isn't "mentioned"
  ];
  assert.deepEqual(sessionPaths(vault), ["Sessions/Session 10.md", "Sessions/Session 2.md", "Sessions/Session 1.md"]);
  assert.deepEqual(recentlyMentioned(vault), [
    { path: "NPCs/Mirela.md", kind: "npc", count: 3 },
    { path: "Locations/Phandalin.md", kind: "location", count: 2 },
    { path: "Items/Crow Signet.md", kind: "item", count: 1 },
    { path: "Factions/The Crows.md", kind: "faction", count: 1 },
  ]);
  assert.equal(recentlyMentioned(vault, 2).length, 2);
  assert.deepEqual(recentlyMentioned([]), []);
});

test("renaming a page rewrites the links to it, and only those", async () => {
  const { renameLinks } = await import("./vault.js");
  const md = "@[[Mirela]] met [[mirela|the elf]] about [[Mirela#Secrets]], ![[NPCs/Mirela]] ![[Mirela.md]]; not [[Older]] or [[Mirelas]] or [[Archive/NPCs/Mirela]]";
  assert.equal(
    renameLinks(md, "NPCs/Mirela.md", "NPCs/Mira.md", paths),
    "@[[Mira]] met [[Mira|the elf]] about [[Mira#Secrets]], ![[NPCs/Mira]] ![[Mira.md]]; not [[Older]] or [[Mirelas]] or [[Archive/NPCs/Mirela]]",
  );
  // The archived copy: only links that resolve to it change.
  assert.equal(renameLinks(md, "Archive/NPCs/Mirela.md", "Archive/NPCs/Old Mirela.md", paths), md.replace("[[Archive/NPCs/Mirela]]", "[[Archive/NPCs/Old Mirela]]"));
  // Case-only rename, and a new name that a shorter path already has: the link keeps pointing at the renamed page.
  assert.equal(renameLinks("[[mirela]] [[MIRELA|x]]", "NPCs/Mirela.md", "NPCs/mirela.md", paths), "[[mirela]] [[mirela|x]]");
  assert.equal(renameLinks("[[Phandalin]]", "Locations/Phandalin.md", "Locations/Mirela.md", paths), "[[Locations/Mirela]]");
  assert.equal(renameLinks("[[Mirela]]", "NPCs/Mirela.md", "NPCs/Ca$$ Shop.md", paths), "[[Ca$$ Shop]]");
  assert.equal(renameLinks("no links", "NPCs/Mirela.md", "NPCs/Mira.md", paths), "no links");
});
