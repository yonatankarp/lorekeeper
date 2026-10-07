// Pure vault logic: tree, [[link]] resolution, backlinks, search, templates. No DOM, tested in node.

// [[Target]], [[Target#Heading]], [[Target|Label]], ![[Embed]]
export const WIKILINK = /(!?)\[\[([^\]|#]*)(#[^\]|]*)?(?:\|([^\]]*))?\]\]/g;

export const baseName = (path) => path.split("/").pop().replace(/\.md$/i, "");
export const naturally = (a, b) => a.localeCompare(b, undefined, { numeric: true, sensitivity: "base" });

/** Splits leading `---` properties from the body. Values are kept as raw strings. */
export function splitFrontmatter(md) {
  const m = md.match(/^---\r?\n([\s\S]*?)\r?\n---[ \t]*(?:\r?\n|$)/);
  if (!m) return { props: [], body: md };
  const props = m[1]
    .split(/\r?\n/)
    .map((line) => line.match(/^([^:#\s][^:]*):\s?(.*)$/))
    .filter(Boolean)
    .map(([, key, value]) => [key.trim(), value.trim()]);
  return { props, body: md.slice(m[0].length) };
}

/** Obsidian-style: [[Name]] matches a file name anywhere, [[Folder/Name]] a path suffix; shortest path wins. */
export function resolve(target, paths) {
  const t = target.trim().replace(/\.md$/i, "").toLowerCase();
  if (!t) return null;
  const hits = paths.filter((p) => {
    const q = p.replace(/\.md$/i, "").toLowerCase();
    return q === t || q.endsWith("/" + t);
  });
  return hits.sort((a, b) => a.length - b.length || naturally(a, b))[0] ?? null;
}

/** Nested { name, path, dirs, files } from folder paths and note paths. Sessions/ lists newest first. */
export function buildTree(folders, paths) {
  const root = { name: "", path: "", dirs: [], files: [] };
  const dirAt = (path) => {
    let node = root;
    for (const part of path.split("/").filter(Boolean)) {
      let next = node.dirs.find((d) => d.name === part);
      if (!next) {
        next = { name: part, path: node.path ? `${node.path}/${part}` : part, dirs: [], files: [] };
        node.dirs.push(next);
      }
      node = next;
    }
    return node;
  };
  folders.forEach(dirAt);
  for (const p of paths) dirAt(p.split("/").slice(0, -1).join("/")).files.push(p);
  const sort = (node) => {
    node.dirs.sort((a, b) => naturally(a.name, b.name)).forEach(sort);
    const order = node.path === "Sessions" ? -1 : 1;
    node.files.sort((a, b) => order * naturally(baseName(a), baseName(b)));
  };
  sort(root);
  return root;
}

/** Notes linking to `path`, each with the lines that contain the link. */
export function backlinks(path, notes) {
  const paths = notes.map((n) => n.path);
  const result = [];
  for (const note of notes) {
    if (note.path === path) continue;
    const lines = note.content.split("\n").filter((line) =>
      [...line.matchAll(WIKILINK)].some((m) => resolve(m[2], paths) === path),
    );
    if (lines.length) result.push({ path: note.path, lines: lines.map((l) => l.trim()) });
  }
  return result.sort((a, b) => naturally(a.path, b.path));
}

/** Case-insensitive search over names and content; name matches first. */
export function search(query, notes) {
  const q = query.trim().toLowerCase();
  if (!q) return [];
  const hits = [];
  for (const note of notes) {
    const inName = baseName(note.path).toLowerCase().includes(q);
    const line = note.content.split("\n").find((l) => l.toLowerCase().includes(q));
    if (inName || line) hits.push({ path: note.path, inName, snippet: line?.trim() ?? "" });
  }
  return hits.sort((a, b) => b.inName - a.inName || naturally(a.path, b.path));
}

/** Obsidian template variables. */
export const fillTemplate = (template, title, date) =>
  template.replaceAll("{{title}}", title).replaceAll("{{date}}", date);

/** Template "NPC" goes to folder "NPCs", or a folder with its own name ("Lore" to "Lore"), when it exists, else the vault root. */
export function folderFor(templateName, folders) {
  const named = (name) => folders.find((f) => f.toLowerCase() === name.toLowerCase());
  return named(`${templateName}s`) ?? named(templateName) ?? "";
}

/** A quest page's `status` property, lowercased and unquoted; missing or empty counts as "open". */
export function questStatus(content) {
  const value = splitFrontmatter(content).props.find(([k]) => k.toLowerCase() === "status")?.[1] ?? "";
  return value.replace(/^["']|["']$/g, "").trim().toLowerCase() || "open";
}

/** Paths of the pages in Quests/ that are still open, sorted by name. */
export const openQuests = (notes) =>
  notes
    .filter((n) => n.path.startsWith("Quests/") && questStatus(n.content) === "open")
    .map((n) => n.path)
    .sort((a, b) => naturally(baseName(a), baseName(b)));

/** Sessions/ pages, newest first (natural order, as in the sidebar). */
export const sessionPaths = (notes) =>
  notes.map((n) => n.path).filter((p) => p.startsWith("Sessions/")).sort((a, b) => naturally(baseName(b), baseName(a)));

// ---------- private notes (sync.rs): your own in Private/, a DM's read-only copies of the players' in .lorekeeper/dm/ ----------

const DM_COPY = /^\.lorekeeper\/dm\/([a-z2-7]{26})\//;

/** A DM's copy of a player's private note: ".lorekeeper/dm/<member id>/NPCs/Vex.md". Read-only. */
export const isDmCopy = (path) => DM_COPY.test(path);

/** The member id whose private note a DM copy is; "" for any other path. */
export const dmCopyOf = (path) => path.match(DM_COPY)?.[1] ?? "";

/** Your own private notes ("Private/...", synced to your private space only) and a DM's copies of the players'. */
export const isPrivate = (path) => path.startsWith("Private/") || isDmCopy(path);

/** A path without its private prefix: "Private/NPCs/Vex.md" and ".lorekeeper/dm/<id>/NPCs/Vex.md" are "NPCs/Vex.md". */
export const unprivate = (path) => path.replace(/^Private\//, "").replace(DM_COPY, "");

/**
 * Who reads your private notes in a campaign (its sharing settings), as every label says it: the server's last word on
 * the campaign's setting (sync.rs Access), so it's always the truth. "" outside a shared campaign. A campaign on a
 * server that hasn't said yet (just joined, offline) may let the DM read them, and a `~` note written now goes up once
 * it connects, so it says so; only the owner changes the setting, which starts off, so their own campaign is the exception.
 */
export function privateLabel(sharing) {
  if (!sharing?.shared) return "";
  const dmReads = sharing.access ? sharing.access.dmReadsPrivate : !!sharing.room && sharing.role !== "owner";
  return dmReads ? "Private: you and the DM" : "Private: only you";
}

// ---------- shared sessions: Sessions/Session N/ holds one file per player (lib.rs start_file) ----------

const SESSION_FOLDER = /^Sessions\/Session \d+$/i;
const SESSION_PART = /^(Sessions\/Session \d+)(?:\.md|\/[^/]+\.md)$/i;
/** A private file in a session: yours ("Private/Sessions/Session 4/Sibling 5.md") or a DM's copy of a player's. */
const PRIVATE_PART = /^(?:Private\/|\.lorekeeper\/dm\/[a-z2-7]{26}\/)(Sessions\/Session \d+)\/[^/]+\.md$/;

/** Whether `path` is a shared session's folder ("Sessions/Session 4"). */
export const isSessionFolder = (path) => SESSION_FOLDER.test(path);

/** A player's file's author: "Sessions/Session 4/Sibling 5.md" (or a private one) is Sibling 5's; "" for any other path. */
export const authorOf = (path) => (/^Sessions\/Session \d+\/[^/]+\.md$/i.test(path) || PRIVATE_PART.test(path) ? baseName(path) : "");

/** `me` (and the author) of a DM who plays no character: their file is "Sessions/Session 4/DM.md" (DM_ME in lib.rs). */
export const DM = "DM";

/**
 * The pages the app shows for the notes on disk (`files`, [{ path, content }]) and the vault's `folders`. A shared session's
 * folder is one page, "Sessions/Session 4", made of its players' files (plus a Session 4.md written next to it); a sync
 * conflict's copy among them is left out (see syncConflicts). Everything else is its own page, the same object.
 * A session page: { path, parts, private, content }, parts being the files and content their text in one (frontmatter
 * from the first, then each body), so links, backlinks, search and the sessions list work on it like on any page.
 * `private` holds the private files of that session (yours, and a DM's copies of the players'): shown in its timeline
 * with a lock, never in its content, and still pages of their own.
 */
export function pages(files, folders = []) {
  const copies = new Set(syncConflicts(files.map((f) => f.path)).map((c) => c.path));
  const ids = new Set(folders.filter(isSessionFolder));
  for (const f of files) {
    const id = f.path.match(SESSION_PART)?.[1] ?? f.path.match(PRIVATE_PART)?.[1];
    if (id && f.path !== `${id}.md`) ids.add(id);
  }
  const partOf = (f) => {
    const id = f.path.match(SESSION_PART)?.[1];
    return id && ids.has(id) ? id : null;
  };
  const out = files.filter((f) => !partOf(f));
  for (const id of ids) {
    const parts = files.filter((f) => partOf(f) === id && !copies.has(f.path)).sort((a, b) => naturally(a.path, b.path)); // Session 4.md first
    const secret = files.filter((f) => f.path.match(PRIVATE_PART)?.[1] === id && !copies.has(f.path)).sort((a, b) => naturally(a.path, b.path));
    out.push({
      path: id,
      parts,
      private: secret,
      get content() {
        const date = parts.map((p) => splitFrontmatter(p.content).props.find(([k]) => k.toLowerCase() === "date")?.[1]).find(Boolean);
        const head = `---\nsession: ${id.replace(/^.* /, "")}\n${date ? `date: ${date}\n` : ""}---\n`;
        return head + parts.map((p) => splitFrontmatter(p.content).body).join("\n");
      },
    });
  }
  return out;
}

/** "PCs/Sibling 5.md", the PC page named `name` (any case, any folder in PCs/), or null. */
export const pcPath = (name, paths) => paths.find((p) => p.startsWith("PCs/") && baseName(p).toLowerCase() === name.toLowerCase()) ?? null;

/**
 * Copies a sync app made when two computers changed a file at once: [{ path, of }] with `of` the original's path, or "" when
 * the name doesn't say. Dropbox: "Vex (Mirela's conflicted copy 2026-10-06).md"; Syncthing: "Vex.sync-conflict-...md";
 * "(Conflict ...)" or "[Conflict]" in a name; Google Drive for desktop and others: "Vex (1).md" next to "Vex.md" (1 or 2
 * digits, so a D&D Beyond import's "Arn (12345678).md" isn't one). Works on files and folders alike.
 */
export function syncConflicts(paths) {
  const all = new Set(paths);
  const out = [];
  for (const path of paths) {
    const name = path.split("/").pop();
    const dir = path.slice(0, path.length - name.length);
    const numbered = name.match(/^(.+) \(\d{1,2}\)(\.[^.]+)?$/);
    if (numbered && all.has(dir + numbered[1] + (numbered[2] ?? ""))) out.push({ path, of: dir + numbered[1] + (numbered[2] ?? "") });
    else if (/conflicted copy|[([][^)\]]*\bconflict|\.sync-conflict-/i.test(name)) {
      const of = dir + name.replace(/ ?(\([^)]*conflict[^)]*\)|\[[^\]]*conflict[^\]]*\]|\.sync-conflict-[^.]*)/i, "");
      out.push({ path, of: all.has(of) ? of : "" });
    }
  }
  return out.sort((a, b) => naturally(a.path, b.path));
}

/** A property's value, by case-insensitive key, unquoted; "" when missing. */
const prop = (props, key) => (props.find(([k]) => k.toLowerCase() === key)?.[1] ?? "").replace(/^["']|["']$/g, "").trim();

/** The PCs/ pages with their class, race, level and player properties, sorted by name. */
export const party = (notes) =>
  notes
    .filter((n) => n.path.startsWith("PCs/"))
    .map((n) => {
      const { props } = splitFrontmatter(n.content);
      return { path: n.path, ...Object.fromEntries(["class", "race", "level", "player"].map((k) => [k, prop(props, k)])) };
    })
    .sort((a, b) => naturally(baseName(a.path), baseName(b.path)));

/** The kind of page a folder holds, for its icon ("NPCs/Vex.md" is an npc). */
const FOLDER_KINDS = { NPCs: "npc", PCs: "pc", Locations: "location", Items: "item", Factions: "faction", Quests: "quest", Lore: "lore" };
export const kindOf = (path) => FOLDER_KINDS[path.split("/")[0]] ?? "note";

/** Icons for well-known properties; a value naming a page shows that page's icon instead. */
const PROP_ICONS = { category: "lore", reward: "loot", rarity: "item", giver: "npc", leader: "npc", owner: "npc", player: "pc", location: "location", base: "location", region: "location" };

/** Labels that read better than the property's name. */
const PROP_LABELS = { player: "Played by", "first-met": "First met", dndbeyond: "D&D Beyond" };
const capital = (s) => s.replace(/^./, (c) => c.toUpperCase());

/** The line that sums up a PC ("Human Barbarian, level 1") or an NPC ("Human, miner's exchange"), and the keys it uses. */
function summary(kind, props) {
  const get = (k) => prop(props, k);
  if (kind === "pc") {
    const level = get("level") && `level ${get("level")}`;
    return { text: capital([[get("race"), get("class")].filter(Boolean).join(" "), level].filter(Boolean).join(", ")), keys: ["race", "class", "level"] };
  }
  if (kind === "npc") return { text: capital([get("race"), get("role")].filter(Boolean).join(", ")), keys: ["race", "role"] };
  return { text: "", keys: [] };
}

/**
 * A page's properties as shown above it, for a page of `kind` (see kindOf; its `type` property wins): a summary line
 * for PCs and NPCs, then rows with `type`, empty ones and the summed-up ones left out (the page already says what it
 * is), labels in words ("first-met" is "First met"), and a value that names a page linked to it.
 * Each row: { key, label, value, icon, path } with path "" when the value isn't a page.
 */
export function shownProps(props, paths, kind = "note") {
  const sum = summary(prop(props, "type").toLowerCase() || kind, props);
  const rows = props
    .map(([key, raw]) => [key, raw.replace(/^["']|["']$/g, "").trim()])
    .filter(([key, value]) => value && key.toLowerCase() !== "type" && !sum.keys.includes(key.toLowerCase()))
    .map(([key, value]) => {
      const k = key.toLowerCase();
      const path = value.includes("[[") ? "" : resolve(value, paths) ?? "";
      const label = PROP_LABELS[k] ?? capital(key.replace(/[-_]+/g, " "));
      return { key: k, label, value, icon: path ? kindOf(path) : PROP_ICONS[k] ?? "", path };
    });
  return { summary: sum.text, rows };
}

const MENTIONED = { NPCs: "npc", Locations: "location", Items: "item", Factions: "faction", Lore: "lore" };

/** NPC, Location, Item, Faction and Lore pages linked from the latest two sessions: [{ path, kind, count }], most mentioned first. */
export function recentlyMentioned(notes, n = 8) {
  const paths = notes.map((note) => note.path);
  const counts = new Map();
  for (const session of sessionPaths(notes).slice(0, 2)) {
    for (const m of notes.find((note) => note.path === session).content.matchAll(WIKILINK)) {
      const path = resolve(m[2], paths);
      if (path && MENTIONED[path.split("/")[0]]) counts.set(path, (counts.get(path) ?? 0) + 1);
    }
  }
  return [...counts]
    .map(([path, count]) => ({ path, kind: MENTIONED[path.split("/")[0]], count }))
    .sort((a, b) => b.count - a.count || naturally(baseName(a.path), baseName(b.path)))
    .slice(0, n);
}

/** Characters that break Obsidian links or file names. Returns an error message, or "" when fine. */
export function badName(name) {
  if (!name.trim()) return "Give the page a name.";
  if (/[\\/:*?"<>|#^[\]]/.test(name)) return 'Names can\'t contain \\ / : * ? " < > | # ^ [ ]';
  if (name.trim().startsWith(".")) return "Names can't start with a dot.";
  return "";
}

/** Where page `path` goes when moved into `folder` ("" is the top level). */
export const movedPath = (path, folder) => `${folder ? `${folder}/` : ""}${path.split("/").pop()}`;

/** Why a session can't be renamed or moved (rename_note in lib.rs). */
export const SESSIONS_STAY = 'Sessions keep their "Session N" names in Sessions/, so hotkey notes find the current one.';
/** Whether page `path` is a session ("Sessions/Session 4.md", a shared one's folder or a file in it): see SESSIONS_STAY. */
export const keepsPlace = (path) => /^Sessions\/Session \d+(\.md|\/|$)/i.test(path);

/**
 * Why page `from` can't move into `folder` ("" is the top level), or "" when it can (`paths`: the vault's pages). The
 * same rules as rename_note in lib.rs, checked before a drop so the reason can be told without trying.
 */
export function moveProblem(from, folder, paths) {
  if (keepsPlace(from)) return SESSIONS_STAY;
  if (/^sessions(\/|$)/i.test(folder) && !/^sessions\//i.test(from)) return "Only sessions go in Sessions/.";
  if (isDmCopy(from)) return "A player's private note is read-only here.";
  const to = movedPath(from, folder);
  // Into or out of Private/ goes through Make private / Make shared, which say who will see it.
  if (isPrivate(from) !== isPrivate(to) || isDmCopy(to)) return isPrivate(from) ? "Use Make shared to share a private page." : "Use Make private to make a page private.";
  if (to !== from && paths.some((p) => p.toLowerCase() === to.toLowerCase())) return `${folder || "The top level"} already has a page named ${baseName(from)}.`;
  return "";
}

/**
 * Points the [[links]] to page `from` at its new path `to` (`paths`: the vault before the rename or move), keeping each
 * link's alias, heading, embed and folder prefix. A bare new name that would land on another page gets its folder, and
 * so does a link to another page that the renamed one would take over (same name, now a shorter path).
 */
export function renameLinks(md, from, to, paths) {
  const after = paths.map((p) => (p === from ? to : p));
  return md.replace(WIKILINK, (link, bang, target, heading = "", alias) => {
    const was = resolve(target, paths);
    if (!was || (was !== from && resolve(target, after) === was)) return link;
    const old = target.trim();
    let name = old.slice(0, old.lastIndexOf("/") + 1) + baseName(to);
    if (was !== from) name = was.replace(/\.md$/i, "");
    else if (resolve(name, after) !== to) name = to.replace(/\.md$/i, "");
    if (/\.md$/i.test(old)) name += ".md";
    return `${bang}[[${target.replace(old, () => name)}${heading}${alias === undefined ? "" : `|${alias}`}]]`;
  });
}

// ---------- D&D Beyond characters ----------

/** Like splitFrontmatter's, and an empty block ("---\n---") counts too. */
const FRONTMATTER = /^---(\r?\n)(?:([\s\S]*?)\r?\n)??(---[ \t]*(?:\r?\n|$))/;
const PROP_LINE = /^([^:#\s][^:]*):\s?(.*)$/;
/** A value as YAML: quoted when it would otherwise mean something else (a leading indicator, ": ", " #"). */
const yamlValue = (v) => (/^[\s\-?:,[\]{}#&*!|>'"%@`]|: | #|\s$/.test(v) ? JSON.stringify(v) : v);

/**
 * `md` with the properties in `updates` ({ key: value }) set: an existing key (any case) keeps its spelling and place and
 * gets the new value, a missing one is added at the end, and frontmatter is made when there is none. Keys in `onlyIfEmpty`
 * are only set when missing or blank. Every other line and the body stay exactly as they were.
 */
export function setProps(md, updates, { onlyIfEmpty = [] } = {}) {
  const m = md.match(FRONTMATTER);
  const nl = m?.[1] ?? (md.includes("\r\n") ? "\r\n" : "\n");
  const lines = m?.[2]?.split(nl) ?? [];
  const todo = new Map(Object.entries(updates).map(([k, v]) => [k.toLowerCase(), [k, String(v)]]));
  const out = [];
  for (let i = 0; i < lines.length; i++) {
    const kv = lines[i].match(PROP_LINE);
    const key = kv?.[1].trim().toLowerCase();
    if (!kv || !todo.has(key)) {
      out.push(lines[i]);
      continue;
    }
    let end = i + 1; // a list or folded text under the key belongs to it
    while (end < lines.length && /^(\s+\S|-(\s|$))/.test(lines[end])) end++;
    const empty = end === i + 1 && !kv[2].replace(/^["']|["']$/g, "").trim();
    out.push(...(onlyIfEmpty.includes(key) && !empty ? lines.slice(i, end) : [`${kv[1].trimEnd()}: ${yamlValue(todo.get(key)[1])}`]));
    todo.delete(key);
    i = end - 1;
  }
  for (const [k, v] of todo.values()) out.push(`${k}: ${yamlValue(v)}`);
  const inner = out.length ? out.join(nl) + nl : "";
  return m ? `---${nl}${inner}${m[3]}${md.slice(m[0].length)}` : `---${nl}${inner}---${nl}${md}`;
}

/** The character id in a D&D Beyond sheet link, or "". */
export const sheetId = (url) => url.match(/^https:\/\/(?:www\.)?dndbeyond\.com\/(?:[^?#]*\/)?characters\/(\d+)/)?.[1] ?? "";

/** The D&D Beyond character id in a page's `dndbeyond` link, or "". */
export const dndBeyondId = (content) => sheetId(prop(splitFrontmatter(content).props, "dndbeyond"));

/**
 * The PCs/ page of a D&D Beyond character ({ id, name }): the one linking to its sheet, else one with its name (any case)
 * that doesn't link to another character's sheet. null when there is none.
 */
export function pcPageFor(character, notes) {
  const pcs = notes.filter((n) => n.path.startsWith("PCs/"));
  const id = String(character.id);
  const named = (n) => baseName(n.path).toLowerCase() === character.name.trim().toLowerCase() && [id, ""].includes(dndBeyondId(n.content));
  return (pcs.find((n) => dndBeyondId(n.content) === id) ?? pcs.find(named))?.path ?? null;
}

/** `name` as a page name: characters badName refuses become spaces; `fallback` when nothing usable is left. */
export function safePageName(name, fallback) {
  const clean = name.replace(/[\\/:*?"<>|#^[\]]/g, " ").replace(/\s+/g, " ").trim().replace(/^\.+\s*/, "");
  return badName(clean) ? fallback : clean;
}

/**
 * `md` with `text` under its `## heading` when that section is still empty: a section with anything written in it is left
 * alone, and a missing one is added before "## Notes" (or at the end).
 */
export function fillSection(md, heading, text) {
  if (!text) return md;
  const nl = md.includes("\r\n") ? "\r\n" : "\n";
  const lines = md.split(nl);
  const block = ["", ...text.split("\n"), ""];
  const at = lines.findIndex((l) => l.trim().toLowerCase() === `## ${heading}`.toLowerCase());
  if (at === -1) {
    const notes = lines.findIndex((l) => /^##\s+notes\s*$/i.test(l));
    if (notes === -1) return `${md.replace(/\s*$/, "")}${nl}${nl}## ${heading}${nl}${block.join(nl)}`;
    lines.splice(notes, 0, `## ${heading}`, ...block);
    return lines.join(nl);
  }
  let end = at + 1;
  while (end < lines.length && !/^#{1,2}\s/.test(lines[end])) end++;
  if (lines.slice(at + 1, end).some((l) => l.trim())) return md;
  lines.splice(at + 1, end - at - 1, ...block);
  return lines.join(nl);
}

/**
 * A PC page brought up to date with a D&D Beyond character: `race`, `class`, `level`, `background`, `alignment` and the
 * `dndbeyond` sheet link are replaced (unless D&D Beyond left them blank); `player` and `portrait` (a saved image's path) are
 * only filled in when empty, so what you set stays. Appearance and Personality are written only into empty sections.
 */
export function characterProps(md, c, portrait = "") {
  const updates = {
    race: c.race, class: c.classes, level: c.level ? String(c.level) : "", background: c.background, alignment: c.alignment,
    dndbeyond: c.url, player: c.player, portrait: portrait && `[[${portrait}]]`,
  };
  const props = setProps(md, Object.fromEntries(Object.entries(updates).filter(([, v]) => v)), { onlyIfEmpty: ["player", "portrait"] });
  return fillSection(fillSection(props, "Appearance", c.appearance), "Personality", c.personality);
}

/**
 * What the main window shows: "first-run" before you have a campaign open (create or join one), "move" (the open
 * campaign, with the offer to move a campaign out of the Lorekeeper folder itself; `offer` is move_offer's folder name),
 * else "campaign".
 */
export const startView = (settings, offer) => (!settings?.vaultPath ? "first-run" : offer ? "move" : "campaign");
