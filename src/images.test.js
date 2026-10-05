import assert from "node:assert/strict";
import test from "node:test";
import { freeName, imageLabel, isImage, pastedName, resolveImage, safeName } from "./images.js";
import { linkify, parse, toHtml, toText } from "./notes.js";

const images = ["map.png", "Attachments/map.png", "Maps/Old/map.png", "Attachments/Pasted image 20261005143012.png", "Handouts/Letter.JPG", "b/x.webp", "a/x.webp"];

test("only the allowed image types count", () => {
  for (const name of ["a.png", "a.JPG", "a.jpeg", "a.gif", "a.webp", "Maps/a.svg"]) assert.ok(isImage(name), name);
  for (const name of ["a.md", "a.png.exe", "png", "a.bmp", ""]) assert.ok(!isImage(name), name);
});

test("resolves embeds like Obsidian", () => {
  assert.equal(resolveImage("map.png", images), "map.png"); // an exact vault path wins
  assert.equal(resolveImage("Maps/Old/map.png", images), "Maps/Old/map.png");
  assert.equal(resolveImage("Old/map.png", images), "Maps/Old/map.png"); // path suffix
  assert.equal(resolveImage("Pasted image 20261005143012.png", images), "Attachments/Pasted image 20261005143012.png");
  assert.equal(resolveImage("pasted%20image%2020261005143012.PNG", images), "Attachments/Pasted image 20261005143012.png"); // ![](...) links are encoded
  assert.equal(resolveImage("letter.jpg", images), "Handouts/Letter.JPG"); // case-insensitive
  assert.equal(resolveImage("../Attachments/map.png", images), "Attachments/map.png"); // relative links still find it
  assert.equal(resolveImage("/map.png", images), "map.png");
  assert.equal(resolveImage("x.webp", images), "a/x.webp"); // shortest, then natural order
  assert.equal(resolveImage("ap.png", images), null); // no partial names
  assert.equal(resolveImage("Letter", images), null);
  assert.equal(resolveImage("100%.png", ["100%.png"]), "100%.png"); // a stray % is part of the name
  assert.equal(resolveImage("", images), null);
});

test("an Attachments folder wins over other folders", () => {
  assert.equal(resolveImage("map.png", ["Maps/map.png", "Attachments/map.png", "Sessions/attachments/map.png"]), "Attachments/map.png");
});

test("embed labels give the width, optional height and alt text", () => {
  assert.deepEqual(imageLabel("300", "map.png"), { alt: "map.png", width: "300", height: "" });
  assert.deepEqual(imageLabel("300x200"), { alt: "", width: "300", height: "200" });
  assert.deepEqual(imageLabel("The map|300"), { alt: "The map", width: "300", height: "" });
  assert.deepEqual(imageLabel("The map"), { alt: "The map", width: "", height: "" });
  assert.deepEqual(imageLabel('300" onerror="x'), { alt: '300" onerror="x', width: "", height: "" }); // sizes are digits only
});

test("pasted images get Obsidian's name, dropped ones a safe free name", () => {
  assert.equal(pastedName(new Date(2026, 9, 5, 14, 30, 12), "image/png"), "Pasted image 20261005143012.png");
  assert.equal(pastedName(new Date(2026, 0, 2, 3, 4, 5), "image/jpeg"), "Pasted image 20260102030405.jpg");
  assert.equal(safeName("map [v2] #1.png"), "map -v2- -1.png");
  assert.equal(freeName("Map.png", images), "Map 1.png"); // map.png is taken, in any folder and case
  assert.equal(freeName("map.png", [...images, "Attachments/map 1.png"]), "map 2.png");
  assert.equal(freeName("new.png", images), "new.png");
});

test("embeds reach the link renderer marked as embeds", () => {
  const seen = [];
  linkify("![[map.png|300]] and [[Mirela]]", (target, label, m) => (seen.push([target, m[1], m[4]]), label));
  assert.deepEqual(seen, [["map.png", "!", "300"], ["Mirela", "", undefined]]);
});

test("Copy for D&D Beyond leaves images out", () => {
  const session = parse("# Session 3\n- 20:01 ![[map.png]]\n- 20:05 Found ![[letter.jpg|200]] on [[Mirela]]\n- 20:06 ![a map](Attachments/map%20two.png)\n- 20:07 #Gold");
  const html = toHtml(session);
  assert.equal(html, "<h2>Session 3</h2><h3>What happened</h3><ul><li>Found on Mirela</li></ul><h3>Loot</h3><ul><li>Gold</li></ul>");
  assert.equal(toText(session), "Session 3\n\nWhat happened\n• Found on Mirela\n\nLoot\n• Gold");
  // An embed of a page (not an image) is kept, as its name.
  assert.ok(toText(parse("- ![[Mirela]] said hi")).endsWith("\n• Mirela said hi"));
});
