// Turns a session file into grouped sections and D&D Beyond-ready HTML.
// Hotkey notes look like "- 21:43 @Mirela the innkeeper"; the first character picks the section.
// Hand-written bullets and paragraphs count too, so nothing typed in the editor is silently dropped.
import { splitFrontmatter, WIKILINK } from "./vault.js";

export const SECTIONS = [
  ["event", "What happened"],
  ["npc", "NPCs"],
  ["loot", "Loot"],
  ["quest", "Quests"],
  ["mystery", "Mysteries"],
  ["quote", "Quotes"],
];

const PREFIX = { "@": "npc", "#": "loot", "!": "quest", "?": "mystery", '"': "quote", "“": "quote" };

/** Every line in file order: { title } for a "# " heading, else a note { time, kind, text }. */
function* lines(md) {
  for (const raw of splitFrontmatter(md).body.split("\n")) {
    const line = raw.trim();
    const bullet = line.match(/^[-*+] (?:(\d{1,2}:\d{2}) )?(.*)$/);
    if (!bullet && line.startsWith("#")) {
      if (line.startsWith("# ")) yield { title: line.slice(2).trim() }; // other headings are just structure
      continue;
    }
    let text = (bullet ? bullet[2] : line).trim();
    if (!text || /^(-{3,}|\*{3,})$/.test(text)) continue;
    const kind = PREFIX[text[0]] ?? "event";
    if (kind !== "event" && kind !== "quote") text = text.slice(1).trim(); // quotes keep their marks
    if (text) yield { time: bullet?.[1] ?? "", kind, text };
  }
}

/** Notes in file order: [{ time, kind, text }]. */
export const timeline = (md) => [...lines(md)].filter((l) => !("title" in l));

export function parse(md) {
  let title = "";
  const groups = Object.fromEntries(SECTIONS.map(([key]) => [key, []]));
  for (const l of lines(md)) {
    if ("title" in l) title ||= l.title;
    else groups[l.kind].push(l.text);
  }
  return { title, groups };
}

export const escape = (s) =>
  s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

/** Plain label for a link: [[Mirela]] -> Mirela, [[Baron Vex|the Baron]] -> the Baron. */
const plainLink = (_, bang, target, heading, label) => label ?? target;
const stripLinks = (text) => text.replace(WIKILINK, plainLink);

const filled = (groups) => SECTIONS.filter(([key]) => groups[key].length);

/**
 * HTML for D&D Beyond. `link(target, label)` renders [[links]] (inputs already escaped);
 * the default writes the label only, so the journal never shows brackets.
 */
export function toHtml({ title, groups }, link = (target, label) => label) {
  const item = (t) => escape(t).replace(WIKILINK, (_, bang, target, heading, label) => link(target, label ?? target));
  const head = title ? `<h2>${escape(stripLinks(title))}</h2>` : "";
  return head + filled(groups)
    .map(([key, label]) => `<h3>${label}</h3><ul>${groups[key].map((t) => `<li>${item(t)}</li>`).join("")}</ul>`)
    .join("");
}

export function toText({ title, groups }) {
  const parts = title ? [stripLinks(title)] : [];
  for (const [key, label] of filled(groups)) parts.push(`${label}\n${groups[key].map((t) => `• ${stripLinks(t)}`).join("\n")}`);
  return parts.join("\n\n");
}
