// Turns a session file into grouped sections and D&D Beyond-ready HTML.
// Hotkey notes look like "- 21:43 @Mirela the innkeeper"; the first character picks the section.
// Hand-written bullets and paragraphs count too, so nothing typed in the editor is silently dropped.
import { baseName, sessionPaths, splitFrontmatter, WIKILINK } from "./vault.js";

export const SECTIONS = [
  ["event", "What happened"],
  ["npc", "NPCs"],
  ["loot", "Loot"],
  ["quest", "Quests"],
  ["mystery", "Mysteries"],
  ["quote", "Quotes"],
];

const PREFIX = { "@": "npc", "#": "loot", "!": "quest", "?": "mystery", '"': "quote", "“": "quote" };

/** Every line in file order: { title } for a "# " heading, else a note { time, kind, text, line } (its line index in the file). */
function* lines(md) {
  const body = splitFrontmatter(md).body;
  const offset = md.slice(0, md.length - body.length).split("\n").length - 1; // lines taken by the properties
  for (const [i, raw] of body.split("\n").entries()) {
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
    if (text) yield { time: bullet?.[1] ?? "", kind, text, line: offset + i };
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

/** `md` without line `i`, which must still read `expected` (else the page changed and nothing is removed). */
export function removeLine(md, i, expected) {
  const lines = md.split("\n");
  if (lines[i] !== expected) throw "That note has changed since, so nothing was deleted";
  lines.splice(i, 1);
  return lines.join("\n");
}

/** `md` with `text` put back as line `i`. */
export function insertLine(md, i, text) {
  const lines = md.split("\n");
  lines.splice(Math.min(i, lines.length), 0, text);
  return lines.join("\n");
}

/** Session pages, newest first: [{ path, title, date, count }] (title without link brackets, count = notes). */
export const sessions = (notes) =>
  sessionPaths(notes).map((path) => {
    const content = notes.find((n) => n.path === path).content;
    const date = splitFrontmatter(content).props.find(([k]) => k.toLowerCase() === "date")?.[1] ?? "";
    return { path, title: stripLinks(parse(content).title) || baseName(path), date: date.replace(/^["']|["']$/g, ""), count: timeline(content).length };
  });

export const escape = (s) =>
  s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

/** Plain label for a link: [[Mirela]] -> Mirela, [[Baron Vex|the Baron]] -> the Baron. */
const plainLink = (_, bang, target, heading, label) => label ?? target;
export const stripLinks = (text) => text.replace(WIKILINK, plainLink);

const filled = (groups) => SECTIONS.filter(([key]) => groups[key].length);

/**
 * Escapes text and renders its [[links]] with `link(target, labelHtml)` (target raw, label escaped).
 * Links are found before escaping, so names like "Old King's Road" or "Salt & Iron" survive.
 */
export function linkify(text, link) {
  let html = "";
  let last = 0;
  for (const m of text.matchAll(WIKILINK)) {
    html += escape(text.slice(last, m.index)) + link(m[2].trim(), escape((m[4] ?? m[2]).trim()));
    last = m.index + m[0].length;
  }
  return html + escape(text.slice(last));
}

/**
 * HTML for D&D Beyond. `link(target, labelHtml)` renders [[links]];
 * the default writes the label only, so the journal never shows brackets.
 */
export function toHtml({ title, groups }, link = (target, label) => label) {
  const item = (t) => linkify(t, link);
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
