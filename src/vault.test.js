import assert from "node:assert/strict";
import test from "node:test";
import {
  backlinks, badName, buildTree, CHOICES, characterProps, dndBeyondId, fillTemplate, fillSection, folderFor, kindOf, openQuests, party, pcPageFor, questStatus,
  recentlyMentioned, resolve, safePageName, search, sessionPaths, setProps, shownProps, splitFrontmatter,
  authorOf, isSessionFolder, pages, pcPath, syncConflicts, isDmCopy, dmCopyOf, isPrivate, unprivate, privateLabel, startView,
  DM, mayPlayDm, myPc, ownSessionFiles, editChoices, editTarget, sessionDate,
} from "./vault.js";

test("private notes: your own in Private/, a DM's read-only copies of the players' in .lorekeeper/dm/", () => {
  const id = "aaaaaaaaaaaaaaaaaaaaaaaaaa";
  const copy = `.lorekeeper/dm/${id}/NPCs/Vex.md`;
  assert.ok(isPrivate("Private/NPCs/Vex.md") && isPrivate(copy));
  assert.ok(!isPrivate("NPCs/Private.md") && !isPrivate("private/NPCs/Vex.md") && !isPrivate(".lorekeeper/dm/x/NPCs/Vex.md"));
  assert.ok(isDmCopy(copy) && !isDmCopy("Private/NPCs/Vex.md"));
  assert.equal(dmCopyOf(copy), id);
  assert.equal(dmCopyOf("NPCs/Vex.md"), "");
  assert.equal(unprivate(copy), "NPCs/Vex.md");
  assert.equal(unprivate("Private/Sessions/Session 4/Arn.md"), "Sessions/Session 4/Arn.md");
  assert.equal(authorOf("Private/Sessions/Session 4/Arn.md"), "Arn");
  assert.equal(authorOf(`.lorekeeper/dm/${id}/Sessions/Session 4/Syloth.md`), "Syloth");
  assert.equal(authorOf("Private/NPCs/Arn.md"), "");
});

test("private labels say who reads your private notes, as the server last said", () => {
  assert.equal(privateLabel(undefined), "");
  assert.equal(privateLabel({ shared: false, access: { dmReadsPrivate: true } }), "", "a campaign of your own");
  assert.equal(privateLabel({ shared: true }), "Private: only you", "not on a server yet");
  assert.equal(privateLabel({ shared: true, room: "r" }), "Private: you and the DM", "on a server that hasn't said who reads them yet");
  assert.equal(privateLabel({ shared: true, room: "r", role: "owner" }), "Private: only you", "the owner's setting starts off");
  assert.equal(privateLabel({ shared: true, access: { role: "player", dmReadsPrivate: false } }), "Private: only you");
  assert.equal(privateLabel({ shared: true, access: { role: "player", dmReadsPrivate: true } }), "Private: you and the DM");
});

test("I play: I'm the DM (no character) is for the owner and DMs, never a player", () => {
  const member = { shared: true, room: "r", role: "member" };
  assert.ok(mayPlayDm({ shared: true }), "sharing it makes you the owner");
  assert.ok(mayPlayDm({ ...member, role: "owner" }), "the owner may run the game without a character");
  assert.ok(mayPlayDm({ ...member, access: { role: "dm" } }));
  assert.ok(!mayPlayDm({ ...member, access: { role: "player" } }), "a player");
  assert.ok(!mayPlayDm(member), "the server hasn't said yet");
  assert.equal(myPc({ ...member, me: DM, access: { role: "player" } }), "", "a player's old DM choice: pick your character");
  assert.equal(myPc({ ...member, me: DM, access: { role: "dm" } }), DM);
  assert.equal(myPc({ ...member, role: "owner", me: DM }), DM);
  assert.equal(myPc({ ...member, me: "PCs/Arn.md", access: { role: "player" } }), "PCs/Arn.md");
  assert.equal(myPc(undefined), "");
});

test("a session page shows its private files in its timeline, never in its content, and they stay pages", () => {
  const id = "aaaaaaaaaaaaaaaaaaaaaaaaaa";
  const files = [
    { path: "Sessions/Session 4/Arn.md", content: "---\nsession: 4\n---\n- 20:00 we arrive" },
    { path: "Private/Sessions/Session 4/Arn.md", content: "- 20:01 Vex is lying" },
    { path: `.lorekeeper/dm/${id}/Sessions/Session 4/Syloth.md`, content: "- 20:02 I stole the gem" },
    { path: "Private/Sessions/Session 5/Arn.md", content: "- 21:00 alone so far" },
    { path: "Private/NPCs/Vex.md", content: "# Vex" },
  ];
  const all = pages(files);
  const s4 = all.find((p) => p.path === "Sessions/Session 4");
  assert.deepEqual(s4.parts.map((p) => p.path), ["Sessions/Session 4/Arn.md"]);
  assert.deepEqual(s4.private.map((p) => p.path), [`.lorekeeper/dm/${id}/Sessions/Session 4/Syloth.md`, "Private/Sessions/Session 4/Arn.md"]);
  assert.ok(!s4.content.includes("Vex is lying") && !s4.content.includes("stole"), "not searchable as the shared page");
  assert.ok(all.some((p) => p.path === "Private/Sessions/Session 4/Arn.md"), "your private file is a page of its own");
  assert.ok(all.some((p) => p.path === "Sessions/Session 5" && p.parts.length === 0 && p.private.length === 1), "a session only you wrote in yet");
  assert.ok(all.some((p) => p.path === "Private/NPCs/Vex.md"));
});

test("the sidebar shows private pages in their usual folders and private session notes in their session", async () => {
  const { sidebarTree, moveTarget, privateMentions } = await import("./vault.js");
  const files = [
    { path: "NPCs/Vex.md", content: "# Vex" },
    { path: "Private/NPCs/Vex.md", content: "# Vex, the real story: works for [[Lorelei]]" },
    { path: "NPCs/Lorelei.md", content: "# Lorelei" },
    { path: "Private/Plots/Heist.md", content: "# Heist" },
    { path: "Sessions/Session 1/Sibling 5.md", content: "- 20:00 we arrive" },
    { path: "Private/Sessions/Session 1/Sibling 5.md", content: "- 20:14 @[[Lorelei]] might be in a secret society" },
    { path: "Private/Sessions/Session 2/Sibling 5.md", content: "- 21:00 alone so far" },
  ];
  const folders = ["NPCs", "Private", "Private/NPCs", "Private/Plots", "Private/Sessions", "Private/Sessions/Session 1", "Private/Sessions/Session 2", "Sessions"];
  const notes = pages(files, folders);
  const tree = sidebarTree(folders.filter((f) => !isSessionFolder(f)), notes.map((n) => n.path));
  const dir = (name) => tree.dirs.find((d) => d.name === name);
  assert.deepEqual(tree.dirs.map((d) => d.name), ["NPCs", "Plots", "Sessions"], "no Private group");
  assert.deepEqual(dir("NPCs").files, ["NPCs/Lorelei.md", "NPCs/Vex.md", "Private/NPCs/Vex.md"], "both Vexes, told apart by isPrivate");
  assert.deepEqual(dir("Plots").files, ["Private/Plots/Heist.md"], "a private-only folder under its own name");
  assert.deepEqual(dir("Sessions").files, ["Sessions/Session 2", "Sessions/Session 1"], "a session with only private notes still shows");
  assert.deepEqual(dir("Sessions").dirs, [], "no private session folders or files of their own");
  const all = JSON.stringify(tree);
  assert.ok(!all.includes("Private/Sessions"), all);
  assert.deepEqual(notes.find((n) => n.path === "Sessions/Session 2").parts, []);
  // Still found where they link, never as backlinks.
  assert.deepEqual(privateMentions("NPCs/Lorelei.md", notes).map((m) => m.path), ["Private/NPCs/Vex.md", "Private/Sessions/Session 1/Sibling 5.md"]);
  assert.deepEqual(backlinks("NPCs/Lorelei.md", notes.filter((n) => !isPrivate(n.path))), []);
  // Moving through the sidebar never crosses the private line: a private page stays in Private/, a shared one out of it.
  assert.equal(moveTarget("Private/NPCs/Vex.md", "Plots"), "Private/Plots");
  assert.equal(moveTarget("Private/NPCs/Vex.md", "Private/Plots"), "Private/Plots");
  assert.equal(moveTarget("Private/NPCs/Vex.md", ""), "Private");
  assert.equal(moveTarget("NPCs/Vex.md", "Private/Plots"), "Plots");
  assert.equal(moveTarget("NPCs/Vex.md", "Locations"), "Locations");
});

test("a private note that links a page is a private mention there, never a backlink", async () => {
  const { privateMentions } = await import("./vault.js");
  const id = "aaaaaaaaaaaaaaaaaaaaaaaaaa";
  const notes = pages([
    { path: "NPCs/Lorelei.md", content: "# Lorelei" },
    { path: "Sessions/Session 1/Sibling 5.md", content: "- 20:10 met [[Lorelei]] at the ferry" },
    { path: "Private/Sessions/Session 1/Sibling 5.md", content: "- 20:14 @[[Lorelei]] might be in a secret society" },
    { path: "Private/NPCs/Vex.md", content: "# Vex\nPays [[NPCs/Lorelei|her]] in gold." },
    { path: `.lorekeeper/dm/${id}/Sessions/Session 1/Syloth.md`, content: "- 20:15 [[Lorelei]] lied to me\n- 20:16 nothing here" },
  ]);
  const shared = notes.filter((n) => !isPrivate(n.path)); // app.js: what Linked from lists on a page that shows private mentions
  assert.deepEqual(backlinks("NPCs/Lorelei.md", shared).map((b) => b.path), ["Sessions/Session 1"]);
  assert.deepEqual(privateMentions("NPCs/Lorelei.md", notes), [
    { path: `.lorekeeper/dm/${id}/Sessions/Session 1/Syloth.md`, lines: ["- 20:15 [[Lorelei]] lied to me"] },
    { path: "Private/NPCs/Vex.md", lines: ["Pays [[NPCs/Lorelei|her]] in gold."] },
    { path: "Private/Sessions/Session 1/Sibling 5.md", lines: ["- 20:14 @[[Lorelei]] might be in a secret society"] },
  ]);
  assert.deepEqual(privateMentions("NPCs/Halia.md", notes), []);
});

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
  assert.equal(folderFor("Lore", ["NPCs", "Lore"]), "Lore");
  assert.equal(folderFor("lore", ["LORE"]), "LORE");
  assert.equal(folderFor("Item", ["Item", "Items"]), "Items");
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
    { path: "Quests/Map the mine.md", content: "---\nstatus: In progress\n---\n" },
    { path: "NPCs/Mirela.md", content: "---\nstatus: open\n---\n" }, // not a quest page
  ];
  assert.deepEqual(openQuests(quests), ["Quests/Map the mine.md", "Quests/Quest 2.md", "Quests/Quest 10.md", "Quests/Rescue Sildar.md"]);
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
    { path: "Sessions/Session 10.md", content: "- [[mirela]] lied, [[Crow Signet]]\n- [[Phandalin#Inn|inn]] [[Mirela]] [[The Old Gods]]" },
    { path: "NPCs/Mirela.md", content: "" },
    { path: "Locations/Phandalin.md", content: "" },
    { path: "Locations/Old Town.md", content: "" },
    { path: "Items/Crow Signet.md", content: "" },
    { path: "Factions/The Crows.md", content: "" },
    { path: "Lore/The Old Gods.md", content: "" },
    { path: "PCs/Pip.md", content: "" }, // the party isn't "mentioned"
  ];
  assert.deepEqual(sessionPaths(vault), ["Sessions/Session 10.md", "Sessions/Session 2.md", "Sessions/Session 1.md"]);
  assert.deepEqual(recentlyMentioned(vault), [
    { path: "NPCs/Mirela.md", kind: "npc", count: 3 },
    { path: "Locations/Phandalin.md", kind: "location", count: 2 },
    { path: "Items/Crow Signet.md", kind: "item", count: 1 },
    { path: "Factions/The Crows.md", kind: "faction", count: 1 },
    { path: "Lore/The Old Gods.md", kind: "lore", count: 1 },
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

test("moving a page keeps plain links, updates path links, and never takes over another page's links", async () => {
  const { keepsPlace, moveProblem, movedPath, renameLinks } = await import("./vault.js");
  const paths = ["NPCs/Vex.md", "Archive/Old/Vex.md", "Lore/Gods.md", "Sessions/Session 3.md", "Sessions/Prep.md"];
  const move = (md, from, folder) => renameLinks(md, from, movedPath(from, folder), paths);
  assert.equal(move("[[Gods]] [[Lore/Gods|gods]] ![[Gods]]", "Lore/Gods.md", "Archive"), "[[Gods]] [[Archive/Gods|gods]] ![[Gods]]");
  // The archived Vex moves to the top level, a shorter path than NPCs/Vex: links to the NPC now name its folder.
  assert.equal(move("[[Vex]] [[vex#Past|she]] [[Archive/Old/Vex]]", "Archive/Old/Vex.md", ""), "[[NPCs/Vex]] [[NPCs/Vex#Past|she]] [[Vex]]");
  // A name another page has, at a shorter path: the moved page's links get its folder.
  assert.equal(move("[[Archive/Old/Vex]]", "Archive/Old/Vex.md", "Places/Far/Away"), "[[Places/Far/Away/Vex]]");
  assert.equal(move("[[Vex]]", "NPCs/Vex.md", "Lore"), "[[Vex]]");

  assert.equal(movedPath("NPCs/Vex.md", ""), "Vex.md");
  assert.equal(moveProblem("Lore/Gods.md", "Archive/Old", paths), "");
  assert.equal(moveProblem("Sessions/Prep.md", "", paths), "");
  // Move never crosses the private line: that's Make private / Make shared, which ask first.
  assert.equal(moveProblem("Private/NPCs/Spy.md", "NPCs", paths), "Use Make shared to share a private page.");
  assert.equal(moveProblem("NPCs/Spy.md", "Private/NPCs", paths), "Use Make private to make a page private.");
  assert.equal(moveProblem("Private/NPCs/Spy.md", "Private/Lore", paths), "");
  assert.equal(moveProblem(".lorekeeper/dm/abcdefghijklmnopqrstuvwxyz/NPCs/Spy.md", "NPCs", paths), "A player's private note is read-only here.");
  assert.match(moveProblem("NPCs/Vex.md", "Archive/Old", paths), /already has a page named Vex/);
  assert.match(moveProblem("Lore/Gods.md", "Sessions", paths), /Only sessions/);
  for (const session of ["Sessions/Session 3.md", "Sessions/Session 4", "Sessions/Session 4/Sibling 5.md"]) {
    assert.match(moveProblem(session, "Lore", paths), /Session N/, session);
    assert.ok(keepsPlace(session), session);
  }
  assert.ok(!keepsPlace("Sessions/Prep.md"));
});

test("shownProps leaves out the type and empty values, names labels, and links page names", () => {
  const paths = ["NPCs/Harbin Wester.md", "Locations/Phandalin.md", "Quests/Umbrage Hill Quest.md"];
  const props = [["type", "quest"], ["status", "open"], ["giver", "Harbin Wester"], ["location", ""], ["reward", "25gp"], ["first-met", '"2026-10-04"'], ["base", "[[Phandalin]]"]];
  assert.deepEqual(shownProps(props, paths, "quest").summary, "");
  assert.deepEqual(shownProps(props, paths, "quest").rows, [
    { key: "status", label: "Status", value: "open", icon: "", path: "" },
    { key: "giver", label: "Giver", value: "Harbin Wester", icon: "npc", path: "NPCs/Harbin Wester.md" },
    { key: "reward", label: "Reward", value: "25gp", icon: "loot", path: "" },
    { key: "first-met", label: "First met", value: "2026-10-04", icon: "", path: "" },
    { key: "base", label: "Base", value: "[[Phandalin]]", icon: "location", path: "" },
  ]);
  assert.equal(kindOf("Lore/Tymora.md"), "lore");

  // PCs and NPCs open with a summary line; its properties aren't repeated as rows.
  const pc = shownProps([["type", "pc"], ["player", "Ofer"], ["class", "Barbarian"], ["race", "Human"], ["level", "1"]], paths, "pc");
  assert.equal(pc.summary, "Human Barbarian, level 1");
  assert.deepEqual(pc.rows.map((r) => [r.label, r.value, r.icon]), [["Played by", "Ofer", "pc"]]);
  const npc = shownProps([["race", "human"], ["role", "Miner's exchange"], ["location", "Phandalin"], ["status", "alive"]], paths, "npc");
  assert.equal(npc.summary, "Human, Miner's exchange");
  assert.deepEqual(npc.rows.map((r) => [r.key, r.path]), [["location", "Locations/Phandalin.md"], ["status", ""], ["attitude", ""]]);
  assert.equal(shownProps([["type", "pc"], ["level", "3"]], paths, "note").summary, "Level 3", "the type property wins over the folder");
  assert.equal(kindOf("Ideas.md"), "note");
});

test("an NPC's status and attitude always show, as unknown until set, so their dropdowns are there", () => {
  const rows = (props, kind = "npc") => shownProps(props, [], kind).rows.map((r) => [r.key, r.label, r.value]);
  // An older page with neither: both rows, at the end.
  assert.deepEqual(rows([["location", "Phandalin"]]), [["location", "Location", "Phandalin"], ["status", "Status", "unknown"], ["attitude", "Attitude", "unknown"]]);
  // The template's empty "attitude:" keeps its place; a value the dropdown doesn't offer is kept as written.
  assert.deepEqual(rows([["Status", "Captured"], ["attitude", ""], ["first-met", "2026-10-04"]]), [["status", "Status", "Captured"], ["attitude", "Attitude", "unknown"], ["first-met", "First met", "2026-10-04"]]);
  assert.deepEqual(rows([["status", "dead"], ["attitude", "'hostile'"]]), [["status", "Status", "dead"], ["attitude", "Attitude", "hostile"]]);
  // Only NPCs: a quest (or a DM's copy of an NPC, which is a plain note) shows no row it doesn't have.
  assert.deepEqual(rows([["giver", "Harbin"]], "quest"), [["giver", "Giver", "Harbin"]]);
  assert.deepEqual(rows([], "note"), []);
  assert.deepEqual(Object.keys(CHOICES.npc), ["status", "attitude"]);
  assert.ok(CHOICES.npc.status.includes("unknown") && CHOICES.npc.attitude.includes("unknown"), "the default is one of the choices");
});

test("setProps changes only the given properties and keeps everything else byte for byte", () => {
  const md = "---\ntype: pc\nRace: elf\n# a comment\nplayer:\naliases:\n  - Dem\n---\n# Demus\n\nrace: not a property\n";
  assert.equal(
    setProps(md, { race: "Hill Dwarf", level: "3" }),
    "---\ntype: pc\nRace: Hill Dwarf\n# a comment\nplayer:\naliases:\n  - Dem\nlevel: 3\n---\n# Demus\n\nrace: not a property\n",
  );
  // CRLF files get CRLF lines; the body is untouched.
  assert.equal(setProps("---\r\ntype: pc\r\n---\r\nBody\r\n", { level: 2 }), "---\r\ntype: pc\r\nlevel: 2\r\n---\r\nBody\r\n");
  // No frontmatter, or an empty block: made, never doubled.
  assert.equal(setProps("# Vex\n", { race: "Elf" }), "---\nrace: Elf\n---\n# Vex\n");
  assert.equal(setProps("", { race: "Elf" }), "---\nrace: Elf\n---\n");
  assert.equal(setProps("---\n---\n# Vex\n---\nmore\n", { race: "Elf" }), "---\nrace: Elf\n---\n# Vex\n---\nmore\n");
  assert.equal(setProps("---\r\n---\r\nBody", { race: "Elf" }), "---\r\nrace: Elf\r\n---\r\nBody");
  // Values that would read as something else in YAML are quoted; a list under a replaced key goes with it.
  assert.equal(setProps("---\nclass:\n  - Old\nx: 1\n---\n", { class: "A: B", race: "#1", note: "[x]" }),
    '---\nclass: "A: B"\nx: 1\nrace: "#1"\nnote: "[x]"\n---\n');
  assert.equal(setProps("---\na: 1\n---\n", { url: "https://www.dndbeyond.com/characters/5" }), "---\na: 1\nurl: https://www.dndbeyond.com/characters/5\n---\n");
  // onlyIfEmpty: missing, blank and "" are empty; a value or a list isn't.
  const only = { onlyIfEmpty: ["player"] };
  assert.equal(setProps("---\nplayer:\n---\n", { player: "x" }, only), "---\nplayer: x\n---\n");
  assert.equal(setProps('---\nplayer: ""\n---\n', { player: "x" }, only), "---\nplayer: x\n---\n");
  assert.equal(setProps("---\ntype: pc\n---\n", { player: "x" }, only), "---\ntype: pc\nplayer: x\n---\n");
  assert.equal(setProps("---\nPlayer: Ofer\n---\n", { player: "x" }, only), "---\nPlayer: Ofer\n---\n");
  assert.equal(setProps("---\nplayer:\n  - Ofer\n---\n", { player: "x" }, only), "---\nplayer:\n  - Ofer\n---\n");
});

test("D&D Beyond characters find their PC page and update only what they own", () => {
  const sheet = "https://www.dndbeyond.com/characters/5";
  assert.equal(dndBeyondId(`---\ndndbeyond: ${sheet}\n---\n`), "5");
  assert.equal(dndBeyondId('---\ndndbeyond: "https://www.dndbeyond.com/profile/x/characters/77/abc"\n---\n'), "77");
  assert.equal(dndBeyondId("---\ndndbeyond: https://evil.com/characters/5\n---\n"), "");
  assert.equal(dndBeyondId("# none"), "");

  const pcs = [
    { path: "PCs/Someone.md", content: `---\ndndbeyond: ${sheet}\n---\n` },
    { path: "PCs/demus.md", content: "---\ntype: pc\n---\n" },
    { path: "PCs/Vex.md", content: "---\ndndbeyond: https://www.dndbeyond.com/characters/9\n---\n" },
    { path: "NPCs/Bob.md", content: "" },
  ];
  assert.equal(pcPageFor({ id: 5, name: "Demus" }, pcs), "PCs/Someone.md", "the sheet link wins over the name");
  assert.equal(pcPageFor({ id: 6, name: " Demus " }, pcs), "PCs/demus.md");
  assert.equal(pcPageFor({ id: 7, name: "Vex" }, pcs), null, "another character's page is never taken");
  assert.equal(pcPageFor({ id: 8, name: "Bob" }, pcs), null, "only PCs/");

  assert.equal(safePageName("Sir: Demus / the #1", "Character 5"), "Sir Demus the 1");
  assert.equal(safePageName("..Vex", "x"), "Vex");
  assert.equal(safePageName(" ?? ", "Character 5"), "Character 5");

  const c = { race: "Hill Dwarf", classes: "Fighter 3 / Rogue 1", level: 4, player: "demus_player", url: sheet };
  const page = "---\ntype: pc\nplayer: Ofer\nclass: Barbarian\nrace:\nlevel: 1\n---\n# Demus\n\n## Backstory\nrace: kept\n";
  assert.equal(characterProps(page, c),
    `---\ntype: pc\nplayer: Ofer\nclass: Fighter 3 / Rogue 1\nrace: Hill Dwarf\nlevel: 4\ndndbeyond: ${sheet}\n---\n# Demus\n\n## Backstory\nrace: kept\n`);
  assert.equal(characterProps("---\nplayer:\nrace: Elf\n---\n", { ...c, race: "", level: 0 }),
    `---\nplayer: demus_player\nrace: Elf\nclass: Fighter 3 / Rogue 1\ndndbeyond: ${sheet}\n---\n`, "blanks from D&D Beyond change nothing");
});

test("the D&D Beyond row is labelled", () => {
  const rows = shownProps([["dndbeyond", "https://www.dndbeyond.com/characters/5"]], [], "pc").rows;
  assert.deepEqual(rows.map((r) => [r.key, r.label, r.path]), [["dndbeyond", "D&D Beyond", ""]]);
});

test("fillSection writes only into an empty section, adding a missing one before Notes", () => {
  const md = "# Demus\n\n## Appearance\n\n## Personality\nGrumpy.\n\n## Notes\n- hi\n";
  assert.equal(fillSection(md, "Appearance", "Tall."), "# Demus\n\n## Appearance\n\nTall.\n\n## Personality\nGrumpy.\n\n## Notes\n- hi\n");
  assert.equal(fillSection(md, "Personality", "Kind."), md, "written already: left alone");
  assert.equal(fillSection(md, "Goals", "Gold."), "# Demus\n\n## Appearance\n\n## Personality\nGrumpy.\n\n## Goals\n\nGold.\n\n## Notes\n- hi\n");
  assert.equal(fillSection("# A\n", "Goals", "Gold."), "# A\n\n## Goals\n\nGold.\n");
  assert.equal(fillSection(md, "Goals", ""), md);
  assert.equal(fillSection("# A\r\n\r\n## Goals\r\n", "Goals", "Gold."), "# A\r\n\r\n## Goals\r\n\r\nGold.\r\n");
});

test("characterProps sets background, alignment and a portrait only when there is none", () => {
  const c = { race: "Human", classes: "Barbarian (Berserker)", level: 1, background: "Folk Hero", alignment: "Chaotic Good", url: "https://www.dndbeyond.com/characters/5", player: "x", appearance: "Tall.", personality: "**Ideals:** Family." };
  const page = "---\ntype: pc\nplayer: Ofer\nportrait:\n---\n# Demus\n\n## Appearance\n\n## Notes\n";
  const out = characterProps(page, c, "Attachments/Demus.jpg");
  assert.match(out, /^---\ntype: pc\nplayer: Ofer\nportrait: "\[\[Attachments\/Demus\.jpg\]\]"\nrace: Human\nclass: Barbarian \(Berserker\)\nlevel: 1\nbackground: Folk Hero\nalignment: Chaotic Good\ndndbeyond: /);
  assert.match(out, /## Appearance\n\nTall\.\n\n## Personality\n\n\*\*Ideals:\*\* Family\.\n\n## Notes\n/);
  assert.match(characterProps(out, c, "Attachments/Demus 2.jpg"), /portrait: "\[\[Attachments\/Demus\.jpg\]\]"/, "an existing portrait stays");
});

test("a shared session's folder is one page, which links, backlinks, search and the tree treat like any page", () => {
  const files = [
    { path: "Sessions/Session 4/Sibling 5.md", content: '---\nsession: 4\ndate: 2026-10-06\nauthor: "[[Sibling 5]]"\n---\n# Session 4 - 2026-10-06\n\n- 20:05 @[[Mirela]] lies\n' },
    { path: "Sessions/Session 4/Arn.md", content: "---\nsession: 4\n---\n# Session 4\n- 20:06 [[Mirela]] again, to [[Phandalin]]\n" },
    { path: "Sessions/Session 4/Arn (1).md", content: "- 20:06 a sync conflict's copy" },
    { path: "Sessions/Session 3.md", content: "# Session 3\n- 19:00 met [[Mirela]]" },
    { path: "NPCs/Mirela.md", content: "# Mirela\nSee [[Session 4]]." },
    { path: "Locations/Phandalin.md", content: "# Phandalin" },
    { path: "PCs/Sibling 5.md", content: "# Sibling 5" },
  ];
  const all = pages(files, ["Sessions", "Sessions/Session 4", "Sessions/Session 6", "NPCs"]);
  const paths = all.map((p) => p.path);
  assert.deepEqual(paths.filter((p) => p.startsWith("Sessions/")).sort(), ["Sessions/Session 3.md", "Sessions/Session 4", "Sessions/Session 6"]);
  const s4 = all.find((p) => p.path === "Sessions/Session 4");
  assert.deepEqual(s4.parts.map((p) => p.path), ["Sessions/Session 4/Arn.md", "Sessions/Session 4/Sibling 5.md"], "no conflict copy");
  assert.equal(s4.parts[0], files[1], "parts are the files themselves, so a saved file shows at once");
  assert.ok(s4.content.startsWith("---\nsession: 4\ndate: 2026-10-06\n---\n"));
  assert.ok(!s4.content.includes("author:"), "only notes in the text, not each file's properties");
  assert.equal(all.find((p) => p.path === "Sessions/Session 6").parts.length, 0, "an empty folder is a session too");
  assert.ok(all.includes(files[4]), "other pages are the same objects");

  assert.equal(resolve("Session 4", paths), "Sessions/Session 4");
  assert.equal(resolve("Sibling 5", paths), "PCs/Sibling 5.md");
  assert.deepEqual(backlinks("NPCs/Mirela.md", all), [
    { path: "Sessions/Session 3.md", lines: ["- 19:00 met [[Mirela]]"] },
    { path: "Sessions/Session 4", lines: ["- 20:06 [[Mirela]] again, to [[Phandalin]]", "- 20:05 @[[Mirela]] lies"] }, // file by file
  ]);
  assert.deepEqual(backlinks("Sessions/Session 4", all).map((b) => b.path), ["NPCs/Mirela.md"]);
  assert.deepEqual(backlinks("PCs/Sibling 5.md", all), [], "authorship isn't a mention");
  assert.deepEqual(sessionPaths(all), ["Sessions/Session 6", "Sessions/Session 4", "Sessions/Session 3.md"]);
  assert.deepEqual(recentlyMentioned(all), [{ path: "NPCs/Mirela.md", kind: "npc", count: 2 }, { path: "Locations/Phandalin.md", kind: "location", count: 1 }]);
  assert.deepEqual(search("again", all).map((h) => h.path), ["Sessions/Session 4"]);
  assert.deepEqual(buildTree(["Sessions"], paths).dirs.find((d) => d.name === "Sessions").files, ["Sessions/Session 6", "Sessions/Session 4", "Sessions/Session 3.md"]);

  assert.equal(authorOf("Sessions/Session 4/Sibling 5.md"), "Sibling 5");
  assert.equal(authorOf("Sessions/Session 4.md"), "");
  assert.equal(authorOf("Sessions/Arc 1/Session 4.md"), "");
  assert.ok(isSessionFolder("Sessions/Session 12") && !isSessionFolder("Sessions/Session 12 (1)") && !isSessionFolder("Sessions/Arc"));
  assert.equal(pcPath("sibling 5", ["NPCs/Sibling 5.md", "PCs/Retired/Sibling 5.md"]), "PCs/Retired/Sibling 5.md");
  assert.equal(pcPath("Vex", paths), null);
});

test("Edit on a shared session: Session notes, My notes and Private notes, each when it applies", () => {
  const f = (path, content = "- 20:00 x\n") => ({ path, content });
  const session = (...files) => pages(files, ["Sessions/Session 1"]).find((p) => p.path === "Sessions/Session 1");
  const old = f("Sessions/Session 1.md", "---\nsession: 1\ndate: 2026-10-04\n---\n# Session 1 - 2026-10-04\n");
  const arn = f("Sessions/Session 1/Arn.md", "---\ndate: 2026-10-05\n---\n- 20:00 x\n");
  const [mine, secret] = ["Sessions/Session 1/Sibling 5.md", "Private/Sessions/Session 1/Sibling 5.md"];
  const ids = (choices) => choices.map((c) => c.id);

  // A session from before sharing, in a shared campaign: all three, its own notes first; files there yet or not.
  const all = editChoices(session(old), "Sibling 5", true);
  assert.deepEqual(all, [{ id: "session", path: old.path }, { id: "mine", path: mine }, { id: "private", path: secret }]);
  assert.equal(editTarget(all, null), old.path, "Edit opens Session notes");
  assert.equal(editTarget(all, "mine"), mine);
  assert.equal(editTarget(all, "private"), secret);
  // A folder session: My notes first, then Private notes.
  const folder = editChoices(session(arn), "Sibling 5", true);
  assert.deepEqual(ids(folder), ["mine", "private"]);
  assert.equal(editTarget(folder, null), mine);
  assert.equal(editTarget(folder, "session"), mine, "a choice that isn't there opens the first");
  // Not shared (no private notes): one choice, so no switch, unless there's a Session 1.md too.
  assert.deepEqual(ids(editChoices(session(arn), "Sibling 5")), ["mine"]);
  assert.deepEqual(ids(editChoices(session(old, f(mine)), "Sibling 5")), ["session", "mine"]);
  // No character picked: just the session's first file.
  assert.deepEqual(editChoices(session(old, arn), "", true), [{ id: "session", path: old.path }]);
  assert.deepEqual(editChoices(session(arn), "", true), [{ id: "session", path: arn.path }]);

  // A file added to a session takes its date: its Session 1.md's, else the first part's that has one.
  assert.equal(sessionDate(session(old, arn)), "2026-10-04");
  assert.equal(sessionDate(session(f(mine), arn)), "2026-10-05");
  assert.equal(sessionDate(session(f(mine))), "", "none: today's, then");
});

test("Delete takes a shared session only when every file in it is yours", () => {
  const f = (path) => ({ path, content: "- 20:00 x\n" });
  const session = (...paths) => pages(paths.map(f)).find((p) => p.path === "Sessions/Session 1");
  const mine = "Sessions/Session 1/Sibling 5.md", secret = "Private/Sessions/Session 1/Sibling 5.md";
  assert.deepEqual(ownSessionFiles(session(mine), "Sibling 5"), [mine]);
  assert.deepEqual(ownSessionFiles(session(mine, secret), "Sibling 5"), [mine, secret], "your private notes go with it");
  assert.deepEqual(ownSessionFiles(session(secret), "Sibling 5"), [secret], "only private notes so far");
  assert.deepEqual(ownSessionFiles(session(mine, "Sessions/Session 1.md"), "Sibling 5"), [], "a session from before sharing");
  assert.deepEqual(ownSessionFiles(session(mine, "Sessions/Session 1/Arn.md"), "Sibling 5"), [], "a teammate's notes");
  assert.deepEqual(ownSessionFiles(session(mine, ".lorekeeper/dm/aaaaaaaaaaaaaaaaaaaaaaaaaa/Sessions/Session 1/Arn.md"), "Sibling 5"), [], "a DM's copy of a player's");
  assert.deepEqual(ownSessionFiles(session(mine), "Arn"), [], "someone else's");
  assert.deepEqual(ownSessionFiles(session(mine), ""), [], "no character picked");
  assert.deepEqual(ownSessionFiles(pages([], ["Sessions/Session 1"])[0], "Sibling 5"), [], "an empty folder");
});

test("sync conflict copies are found by their names", () => {
  const paths = [
    "NPCs/Vex.md", "NPCs/Vex (1).md", "NPCs/Vex (Mirela's conflicted copy 2026-10-06).md", "NPCs/Vex.sync-conflict-20261006-201500-ABCDEFG.md",
    "Quests/Border Conflict.md", "Quests/Border Conflict (2).md", "PCs/Arn.md", "PCs/Arn (12345678).md", "NPCs/Bob (1).md",
    "Sessions/Session 5", "Sessions/Session 5 (1)", "Lore/Gods [Conflict].md", "Lore/Old (conflict 2026-10-06).md",
  ];
  assert.deepEqual(syncConflicts(paths), [
    { path: "Lore/Gods [Conflict].md", of: "" },
    { path: "Lore/Old (conflict 2026-10-06).md", of: "" },
    { path: "NPCs/Vex (1).md", of: "NPCs/Vex.md" },
    { path: "NPCs/Vex (Mirela's conflicted copy 2026-10-06).md", of: "NPCs/Vex.md" },
    { path: "NPCs/Vex.sync-conflict-20261006-201500-ABCDEFG.md", of: "NPCs/Vex.md" },
    { path: "Quests/Border Conflict (2).md", of: "Quests/Border Conflict.md" },
    { path: "Sessions/Session 5 (1)", of: "Sessions/Session 5" },
  ]);
});

// The copies Lorekeeper's sync makes (conflict_name in sync.rs; its is_conflict_copy test uses the same names).
test("Lorekeeper sync's conflict copies are flagged next to their original", () => {
  const paths = [
    "NPCs/Vex.md", "NPCs/Vex (conflict 2026-10-06 2015).md", "NPCs/Vex (conflict 2026-10-06 2015 2).md",
    "Attachments/map.png", "Attachments/map (conflict 2026-10-06 2015).png", "Lore/Gone (conflict 2026-10-06 2015).md",
  ];
  assert.deepEqual(syncConflicts(paths), [
    { path: "Attachments/map (conflict 2026-10-06 2015).png", of: "Attachments/map.png" },
    { path: "Lore/Gone (conflict 2026-10-06 2015).md", of: "" },
    { path: "NPCs/Vex (conflict 2026-10-06 2015 2).md", of: "NPCs/Vex.md" },
    { path: "NPCs/Vex (conflict 2026-10-06 2015).md", of: "NPCs/Vex.md" },
  ]);
});

test("the main window starts with a campaign, the first-run view, or the offer to move one", () => {
  assert.equal(startView({ vaultPath: "", campaigns: [] }), "first-run");
  assert.equal(startView({}), "first-run", "no settings yet");
  assert.equal(startView({ vaultPath: "", campaigns: ["/d/Lorekeeper/Strahd"] }, "x"), "first-run", "joined, not opened yet");
  assert.equal(startView({ vaultPath: "/d/Lorekeeper", campaigns: ["/d/Lorekeeper"] }, "My campaign"), "move");
  assert.equal(startView({ vaultPath: "/d/Lorekeeper/Strahd" }, null), "campaign");
});
