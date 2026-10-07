// Turns a session file into a timeline and sections grouped by kind (the Journal view).
// Hotkey notes look like "- 21:43 @Mirela the innkeeper"; the first character picks the section.
// Hand-written bullets and paragraphs count too, so nothing typed in the editor is silently dropped.
import { authorOf, baseName, DM, isPrivate, naturally, pcPath, sessionPaths, splitFrontmatter, WIKILINK } from "./vault.js";
import { IMAGE_EMBED } from "./images.js";

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
    const kind = text.search(IMAGE_EMBED) === 0 ? "event" : PREFIX[text[0]] ?? "event"; // ![[map.png]] isn't a "!" quest
    if (kind !== "event" && kind !== "quote") text = text.slice(1).trim(); // quotes keep their marks
    if (text) yield { time: bullet?.[1] ?? "", kind, text, line: offset + i };
  }
}

/**
 * A quick note that starts with `~` is private: { private, text } with the `~` gone and the rest filed as usual
 * ("~@Halia lies" is a private NPC note). lib.rs private_note decides where it's saved; this is for the note box.
 */
export function privateNote(text) {
  const m = text.match(/^\s*~(.*)$/s);
  return m ? { private: true, text: m[1] } : { private: false, text };
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

// ---------- shared sessions: one file per player in Sessions/Session N/ (see pages in vault.js) ----------

/** "21:43" as minutes; -1 for a line without a time. */
const minutes = (time) => (time ? time.split(":").reduce((h, m) => h * 60 + Number(m)) : -1);

/**
 * Every player's notes in one timeline: timeline() items with `author` (see authorOf) and `path` (the file the note is
 * in), by time, then by author. A line without a time stays after the note before it in its file, and a time that jumps
 * back more than 6 hours is past midnight.
 * ponytail: a player whose first note is after midnight sorts before the others' notes from before it.
 */
export function mergeTimelines(parts) {
  const all = parts.flatMap(({ path, content }) => {
    let at = -1;
    let day = 0;
    return timeline(content).map((n) => {
      const t = minutes(n.time);
      if (t >= 0) {
        if (t + day < at - 360) day += 1440;
        at = t + day;
      }
      return { ...n, author: authorOf(path), path, at };
    });
  });
  return all.sort((a, b) => a.at - b.at || naturally(a.author, b.author));
}

/**
 * parse() for a shared session: the merged notes grouped by kind, each followed by its author as a link to their PC page
 * (`paths`: the vault's pages), "... ([[PCs/Sibling 5|Sibling 5]])", or "... (DM)" for a DM without a character. Image-only
 * notes get no author. A private note (from a file in Private/ or a DM's copy) is { text, lock: true, path }, for toHtml
 * to mark.
 */
export function parseMerged(parts, paths = []) {
  const groups = Object.fromEntries(SECTIONS.map(([key]) => [key, []]));
  for (const { kind, text, author, path } of mergeTimelines(parts)) {
    const page = author && author !== DM && (pcPath(author, paths)?.replace(/\.md$/i, "") ?? author);
    const by = author === DM ? DM : page && `[[${page}|${author}]]`;
    const shown = by && text.replace(IMAGE_EMBED, "").trim() ? `${text} (${by})` : text;
    groups[kind].push(isPrivate(path) ? { text: shown, lock: true, path } : shown);
  }
  return { title: parts.map((p) => parse(p.content).title).find(Boolean) ?? "", groups };
}

/** The start of `pc`'s own file in shared session folder `folder`, as lib.rs writes it (start_file). */
export function playerFile(folder, pc, date) {
  const n = baseName(folder).replace(/^Session /, "");
  const author = pc === DM ? DM : `"[[${pc}]]"`; // the DM has no PC page to link to
  return `---\nsession: ${n}\ndate: ${date}\nauthor: ${author}\n---\n# Session ${n} - ${date}\n\n`;
}

/**
 * A note's author in the timeline, as HTML: a link to `page` (their PC page, or their name) with `picHtml` (their
 * portrait) before the name; the DM without a character is plain "DM", with no link or portrait.
 */
export const authorLink = (author, page, picHtml = "") =>
  author === DM ? `<span class="author">${DM}</span>` : `<a class="wikilink author" data-target="${escape(page)}" href="#">${picHtml}${escape(author)}</a>`;

/**
 * Escapes text and renders its [[links]] with `link(target, labelHtml, match)` (target raw, label escaped; match[1]
 * is "!" for an embed, match[4] the raw label).
 * Links are found before escaping, so names like "Old King's Road" or "Salt & Iron" survive.
 */
export function linkify(text, link) {
  let html = "";
  let last = 0;
  for (const m of text.matchAll(WIKILINK)) {
    html += escape(text.slice(last, m.index)) + link(m[2].trim(), escape((m[4] ?? m[2]).trim()), m);
    last = m.index + m[0].length;
  }
  return html + escape(text.slice(last));
}

/**
 * The Journal view: a recap grouped by kind. `link(target, labelHtml, match)` renders [[links]] and ![[embeds]] (see
 * linkify); the default writes the label only, so it never shows brackets. A private note ({ text, lock }) gets
 * `lockHtml(item)` after it.
 */
export function toHtml({ title, groups }, link = (target, label) => label, lockHtml = () => " (private)") {
  const item = (t) => (typeof t === "string" ? linkify(t, link) : linkify(t.text, link) + lockHtml(t));
  const head = title ? `<h2>${escape(stripLinks(title))}</h2>` : "";
  return head + filled(groups)
    .map(([key, label]) => `<h3>${label}</h3><ul>${groups[key].map((t) => `<li>${item(t)}</li>`).join("")}</ul>`)
    .join("");
}
