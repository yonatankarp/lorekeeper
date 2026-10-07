// Turns a session file into a timeline and sections grouped by kind (the Journal view).
// Hotkey notes look like "- 21:43 @Mirela the innkeeper"; the first character picks the section.
// Hand-written bullets and paragraphs count too, so nothing typed in the editor is silently dropped.
import { authorOf, baseName, DM, isPrivate, kindOf, naturally, pcPath, resolve, sessionPaths, splitFrontmatter, WIKILINK } from "./vault.js";
import { IMAGE_EMBED } from "./images.js";

export const SECTIONS = [
  ["event", "What happened"],
  ["npc", "NPCs"],
  ["loot", "Loot"],
  ["quest", "Quests"],
  ["mystery", "Mysteries"],
  ["quote", "Quotes"],
];

/** The first character of a note that files it (the cheat sheet lists each, cheatsheet.test.js checks). */
export const PREFIX = { "@": "npc", "#": "loot", "!": "quest", "?": "mystery", '"': "quote", "“": "quote" };

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

/** A line typed private: `~` first, after an optional list marker and "HH:MM " time ("~~gone~~" is strikethrough). */
const PRIVATE_LINE = /^\s*((?:[-*+] )?(?:\d{1,2}:\d{2} )?)~(?!~)\s*(\S.*?)\r?$/;

/**
 * `md` from the editor with its `~` lines taken out, before it's saved to a shared file (app.js save): { shared, private,
 * taken }. `private` holds each line as your private notes get it, the `~` gone and its list marker and time kept (none
 * added); `taken` the lines as typed. Frontmatter and fenced code blocks are left alone, and so is a `~` anywhere but
 * a line's start. For page `page` (not a session; `paths`: the vault's pages), a line that doesn't link it gets a link
 * first, `@[[Halia]]` on an NPC, so it shows in that page's Private notes.
 */
export function splitPrivate(md, page = "", paths = []) {
  const body = splitFrontmatter(md).body;
  const out = { shared: [], private: [], taken: [] };
  const name = page && (resolve(baseName(page), paths) === page ? baseName(page) : page.replace(/\.md$/i, ""));
  const link = page && `${kindOf(page) === "npc" ? "@" : ""}[[${name}]] `;
  let code = false;
  for (const piece of body.split(/(?<=\n)/)) { // each line with its line break, which goes with it
    const line = piece.replace(/\n$/, "");
    if (/^\s*(```|~~~)/.test(line)) code = !code;
    const m = !code && line.match(PRIVATE_LINE);
    if (!m) {
      out.shared.push(piece);
      continue;
    }
    const linked = [...m[2].matchAll(WIKILINK)].some((l) => resolve(l[2], paths) === page);
    out.taken.push(line);
    out.private.push(m[1] + (page && !linked ? link : "") + m[2]);
  }
  return { ...out, shared: md.slice(0, md.length - body.length) + out.shared.join("") };
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

const DAY = 86400000;
/** This computer's UTC offset at moment `ms` (ms since 1970), in minutes east: Berlin in summer is 120. Tests pass their own. */
export const localZone = (ms) => -new Date(ms).getTimezoneOffset();
/** A property's value without quotes; "" when there's none. */
const prop = (md, key) => (splitFrontmatter(md).props.find(([k]) => k.toLowerCase() === key)?.[1] ?? "").replace(/^["']|["']$/g, "");
/** Minutes past midnight as "HH:MM", on any day. */
const clock = (m) => {
  const d = ((m % 1440) + 1440) % 1440;
  return `${String(Math.floor(d / 60)).padStart(2, "0")}:${String(d % 60).padStart(2, "0")}`;
};
/** 120 -> "+02:00", -210 -> "-03:30": the `utc-offset` property (see utcOffset). */
export const formatOffset = (min) => `${min < 0 ? "-" : "+"}${clock(Math.abs(min))}`;

/**
 * A session file's `utc-offset` property ("+02:00", "-03:30", lib.rs start_file) in minutes east of UTC: its writer's
 * zone. null when it has none (notes from before times had zones, or a file made by hand): the viewer's own zone, then.
 */
export function utcOffset(md) {
  const m = prop(md, "utc-offset").match(/^([+-])(\d{2}):(\d{2})$/);
  return m ? (m[1] === "-" ? -1 : 1) * (Number(m[2]) * 60 + Number(m[3])) : null;
}

/** Midnight UTC of session file `md`'s `date` (else of `date`, the session's; else today), in ms. */
function dayOf(md, date = "") {
  for (const d of [prop(md, "date"), date]) {
    const m = d.match(/^(\d{4})-(\d{2})-(\d{2})/);
    if (m) return Date.UTC(m[1], m[2] - 1, m[3]);
  }
  return Date.now() - (Date.now() % DAY);
}

/**
 * A note written at `time`, `t` minutes after midnight of day `start` (more than 1440 past midnight), in a file at
 * `offset` (null: the viewer's zone): { at: its moment in ms, shown: its time in the viewer's `zone` }. `shown` is
 * `time` as written when the zones match, so nothing changes for a party in one zone. A time that lands on another day
 * shows as its clock time ("00:15"): the order around it says which day.
 */
function moment(time, t, start, offset, zone) {
  const own = offset ?? zone(start + t * 60000);
  const at = start + (t - own) * 60000;
  const here = zone(at);
  return { at, shown: own === here ? time : clock(t - own + here) };
}

/** Time `time` from a line of session file `md`, as this computer shows it (see moment). */
export const localTime = (time, md, zone = localZone) => (time ? moment(time, minutes(time), dayOf(md), utcOffset(md), zone).shown : time);

/**
 * timeline() of session file `md`, each note with `at` and `shown` (see moment): its moment is the file's `date` (else
 * `date`, the session's) + its time + a day for each time that jumps back more than 6 hours (past midnight) - the file's
 * `utc-offset`. A line without a time takes the `at` of the note before it (-1 before the first) and shows no time.
 */
export function zoned(md, zone = localZone, date = "") {
  const start = dayOf(md, date);
  const offset = utcOffset(md);
  let at = -1;
  let last = -Infinity;
  let day = 0;
  return timeline(md).map((n) => {
    const t = minutes(n.time);
    if (t < 0) return { ...n, at, shown: "" };
    if (t + day < last - 360) day += 1440;
    last = t + day;
    const m = moment(n.time, last, start, offset, zone);
    at = m.at;
    return { ...n, ...m };
  });
}

/**
 * Every player's notes in one timeline: zoned() items with `author` (see authorOf) and `path` (the file the note is in),
 * by moment, then by author, so notes from different time zones fall in the order they were written. A line without a
 * time stays after the note before it in its file. A file added to a session takes the session's date, which may not be
 * its writer's (New York's evening is past midnight in Berlin), so each file moves by whole days to start within 12 hours
 * of the first file's: one session is one evening. That also puts a player whose first note is past midnight after the
 * others' notes from before it.
 * ponytail: files whose first notes are 12 or more hours apart (prep notes the morning before) land on the same evening;
 * record each file's own start date if sessions ever span days.
 */
export function mergeTimelines(parts, zone = localZone) {
  const date = parts.map((p) => prop(p.content, "date")).find(Boolean) ?? "";
  let first;
  const all = parts.flatMap(({ path, content }) => {
    const notes = zoned(content, zone, date);
    const start = notes.find((n) => n.at >= 0)?.at;
    first ??= start;
    const shift = start === undefined ? 0 : Math.round((first - start) / DAY) * DAY;
    return notes.map((n) => ({ ...n, at: n.at < 0 ? n.at : n.at + shift, author: authorOf(path), path }));
  });
  return all.sort((a, b) => a.at - b.at || naturally(a.author, b.author));
}

/**
 * parse() for a shared session: the merged notes grouped by kind, each followed by its author as a link to their PC page
 * (`paths`: the vault's pages), "... ([[PCs/Sibling 5|Sibling 5]])", or "... (DM)" for a DM without a character. Image-only
 * notes get no author. A private note (from a file in Private/ or a DM's copy) is { text, lock: true, path }, for toHtml
 * to mark.
 */
export function parseMerged(parts, paths = [], zone = localZone) {
  const groups = Object.fromEntries(SECTIONS.map(([key]) => [key, []]));
  for (const { kind, text, author, path } of mergeTimelines(parts, zone)) {
    const page = author && author !== DM && (pcPath(author, paths)?.replace(/\.md$/i, "") ?? author);
    const by = author === DM ? DM : page && `[[${page}|${author}]]`;
    const shown = by && text.replace(IMAGE_EMBED, "").trim() ? `${text} (${by})` : text;
    groups[kind].push(isPrivate(path) ? { text: shown, lock: true, path } : shown);
  }
  return { title: parts.map((p) => parse(p.content).title).find(Boolean) ?? "", groups };
}

/** The start of `pc`'s own file in shared session folder `folder`, as lib.rs writes it (start_file); `offset`: see utcOffset. */
export function playerFile(folder, pc, date, offset = localZone(Date.now())) {
  const n = baseName(folder).replace(/^Session /, "");
  const author = pc === DM ? DM : `"[[${pc}]]"`; // the DM has no PC page to link to
  return `---\nsession: ${n}\ndate: ${date}\nutc-offset: ${formatOffset(offset)}\nauthor: ${author}\n---\n# Session ${n} - ${date}\n\n`;
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
