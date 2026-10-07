// The Lorekeeper window: browse the vault, read and edit pages, follow [[links]], search, backlinks.
import { marked } from "./vendor/marked.esm.js";
import { createEditor } from "./editor.js";
import { authorLink, escape, insertLine, linkify, localTime, mergeTimelines, parse, parseMerged, playerFile, removeLine, sessions, splitPrivate, stripLinks, toHtml, zoned } from "./notes.js";
import {
  backlinks, badName, baseName, buildTree, characterProps, dndBeyondId, fillTemplate, folderFor, isSessionFolder, openQuests, pages, party, pcPageFor,
  pcPath, QUEST_STATUSES, questStatus, recentlyMentioned, setProps, renameLinks, keepsPlace, moveProblem, movedPath, SESSIONS_STAY, naturally, kindOf, resolve, safePageName, search, sheetId, shownProps, splitFrontmatter, syncConflicts,
  DM, dmCopyOf, editChoices, editTarget, isDmCopy, isPrivate, moveTarget, myPc, ownSessionFiles, privateLabel, privateMentions, sessionDate, shownAt, sidebarTree, startView, unprivate,
} from "./vault.js";
import { navHistory, undoStack } from "./history.js";
import { applyTheme, nativeTheme } from "./theme.js";
import { icon } from "./icons.js";
import { GRAPH_KINDS, connections, neighbourhood } from "./graph.js";
import { drawGraph, forgetPictures } from "./graph-view.js";
import { ATTACHMENTS, freeName, imageLabel, isImage, imageTarget, pastedName, resolveImage, safeName } from "./images.js";
import { onlineText, syncText } from "./sync-status.js";
import { cheatSheetHtml, globalKeys, NOTE_PREFIXES, readable } from "./cheatsheet.js";

const { invoke, convertFileSrc } = window.__TAURI__.core;
const { listen, emitTo } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);
const { platform } = document.documentElement.dataset; // set by platform.js
const mac = platform === "macos";
const windows = platform === "windows";
const linux = !mac && !windows;
const tauriWindow = window.__TAURI__.window?.getCurrentWindow();

// notes: the pages shown (a shared session's folder is one, see pages in vault.js); files: every note as on disk.
let vault = { folders: [], notes: [], files: [], conflicts: [], currentSession: "" };
let templates = []; // Templates/ notes: used by "New page", hidden everywhere else
let current = null; // open page path; null is Home
const nav = navHistory(); // pages visited, for Back / Forward
const undos = undoStack(); // the app's own undoable actions (deleting, creating); text edits use the editor's history
let base = ""; // file content the editor started from, for conflict-safe saves
let editing = false;
let editChoice = null; // Edit on a shared session: which file (editChoices: "session", "mine", "private"); null is the first
let saveTimer = null;
let saving = null; // in-flight save promise
let held = false; // unsaved while the cursor is still on a ~ line, or after its move failed (see save): the editor keeps it
let inConflict = false;
const closedFolders = new Set();
const KINDS = { npc: "NPC", loot: "Loot", quest: "Quest", mystery: "Mystery", quote: "Quote" }; // a note's badge; symbols: NOTE_PREFIXES
let settings = { theme: "system", editorFontSize: 15 }; // until get_settings answers
let sessionView = "timeline"; // or "journal": a recap grouped by kind; the switch changes it until restart
let editor = null; // created on first Edit, then reused for every page

/** A page, or a file inside a shared session (a player's file, a sync conflict's copy). */
const note = (path) => vault.notes.find((n) => n.path === path) ?? vault.files.find((n) => n.path === path);
const paths = () => vault.notes.map((n) => n.path);
/** The page a file is shown as: a player's file is part of its session's page. */
const pageFor = (path) => vault.notes.find((n) => n.parts?.some((p) => p.path === path))?.path ?? path;
/** In a campaign shared with your party, the PC page you play ("PCs/Sibling 5.md"); "" otherwise. */
const me = () => {
  const s = settings.sharing?.[settings.vaultPath];
  return (s?.shared && myPc(s)) || "";
};
/** Your own file in shared session `path` ("Sessions/Session 4/Sibling 5.md"), there yet or not; null for other pages. */
const myFile = (path) => (note(path)?.parts && me() ? `${path}/${baseName(me())}.md` : null);
/** The file Edit changes: in a shared session the one picked in its switch (see editChoices), else the page. */
const editPath = (path = current) => (note(path)?.parts ? editTarget(choicesFor(path), path === current ? editChoice : null) : path);
/** A session's notes in order, times in your zone (see zoned): every player's, with authors, for a shared one, and the private ones you may read. */
const sessionItems = (n) => (n.parts ? mergeTimelines([...n.parts, ...(n.private ?? [])]) : zoned(n.content));
/** The open campaign's sharing settings, and who reads your private notes in it (see privateLabel). */
const sharing = () => settings.sharing?.[settings.vaultPath];
/** What Edit can change on shared session `path`: Session notes, My notes, Private notes (see editChoices). */
const choicesFor = (path) => editChoices(note(path), baseName(me()), !!privateLabel(sharing()));
const ownLabel = () => privateLabel(sharing()) || "Private";
/** A player's name for their private notes a DM reads (sync.rs writes the names); never HTML. */
const playerName = (id) => vault.dmPlayers?.[id] || "A player";
/** What a private note's padlock says: yours, or (on a DM's computer) whose. */
const lockLabel = (path) => (isDmCopy(path) ? `Private: ${playerName(dmCopyOf(path))} and the DM` : ownLabel());
const lockHtml = (path) => `<span class="lock" title="${escape(lockLabel(path))}">${icon("lock")}<span class="sr-only">${escape(lockLabel(path))}</span></span>`;
/** Your private notes file in shared session `path`, there yet or not. */
const myPrivateFile = (path) => (me() ? `Private/${path}/${baseName(me())}.md` : null);
/** The files Delete trashes on a shared session that's all yours (see ownSessionFiles); [] when it isn't. */
const ownSession = (path) => (note(path)?.parts ? ownSessionFiles(note(path), baseName(me())) : []);
const isSession = (path) => path?.startsWith("Sessions/");
const alive = (path) => path === null || !!note(path); // Home, or a page still in the vault
const modal = () => !!document.querySelector("dialog[open]");
const dirty = () => saveTimer !== null || saving !== null || held;
const today = () => new Date().toLocaleDateString("sv-SE"); // YYYY-MM-DD

/** The Markdown editor, created the first time it's needed. */
function ed() {
  if (!editor) {
    editor = createEditor($("editor"), {
      onChange: scheduleSave,
      onFollowLink: followLink,
      onImage: saveImage,
      // Private pages are offered only while editing a private one, so their names don't slip into shared pages.
      pageNames: () => vault.notes.filter((n) => isPrivate(editPath()) || !isPrivate(n.path)).map((n) => baseName(n.path)),
    });
    editor.setFontSize(settings.editorFontSize);
  }
  return editor;
}

/** Reads the vault, keeping Templates/ out of the tree, search, links and backlinks, and Attachments/ (images) out of the tree. */
async function loadVault() {
  const v = await invoke("read_vault");
  const under = (dir) => (p) => p === dir || p.startsWith(`${dir}/`);
  const internal = under("Templates"), images = under(ATTACHMENTS);
  templates = v.notes.filter((n) => internal(n.path));
  const files = v.notes.filter((n) => !internal(n.path));
  const folders = v.folders.filter((f) => !internal(f));
  vault = {
    ...v, files, notes: pages(files, folders),
    folders: folders.filter((f) => !isSessionFolder(f) && !images(shownAt(f))), // a session folder shows as the session's page
    conflicts: syncConflicts([...folders, ...files.map((n) => n.path), ...(v.images ?? [])]),
  };
}

let statusTimer;
function say(msg) {
  $("status").textContent = msg;
  clearTimeout(statusTimer);
  statusTimer = setTimeout(() => ($("status").textContent = ""), 4000);
}

// ---------- markdown ----------

const linkHtml = (target, labelHtml) =>
  `<a class="wikilink${resolve(target, paths()) ? "" : " missing"}" data-target="${escape(target)}" href="#">${labelHtml}</a>`;

/**
 * A vault image (`target` as written in ![[target|label]] or ![label](target)) as an <img> served by the asset
 * protocol, which only reaches the notes folder. Web images aren't loaded (the CSP blocks them): they show as a link.
 */
function imageHtml(target, label) {
  const { alt, width, height } = imageLabel(label, target.split("/").pop());
  if (/^[a-z][a-z0-9+.-]*:/i.test(target)) return `<a href="${escape(target)}">${escape(alt || target)}</a>`;
  const path = resolveImage(target, vault.images ?? []);
  if (!path) return `<span class="image-missing" title="Image not found">${escape(alt || target)}</span>`;
  const root = settings.vaultPath ?? "";
  const sep = root.includes("\\") ? "\\" : "/";
  const src = convertFileSrc(root.replace(/[\\/]+$/, "") + sep + path.split("/").join(sep));
  return `<img src="${escape(src)}" alt="${escape(alt)}"${width ? ` width="${width}"` : ""}${height ? ` height="${height}"` : ""} loading="lazy">`;
}

marked.use({
  gfm: true,
  breaks: true, // Obsidian shows single newlines as line breaks
  renderer: { html: ({ text }) => escape(text), image: ({ href, text }) => imageHtml(href, text) }, // raw HTML in notes is shown, not run
  extensions: [
    {
      name: "wikilink",
      level: "inline",
      start: (src) => src.match(/!?\[\[/)?.index,
      tokenizer(src) {
        const m = /^!?\[\[([^\]|#]*)(#[^\]|]*)?(?:\|([^\]]*))?\]\]/.exec(src);
        if (m) return { type: "wikilink", raw: m[0], target: m[1].trim(), label: (m[3] ?? m[1]).trim(), embed: m[0][0] === "!" };
      },
      renderer: (t) => (t.embed && isImage(t.target) ? imageHtml(t.target, t.label) : linkHtml(t.target, escape(t.label))),
    },
  ],
});

/** A [[link]] made clickable, or an ![[image]] shown (a linkify renderer). */
const inlineLink = (target, label, m) => (m[1] && isImage(target) ? imageHtml(target, m[4] ?? target) : linkHtml(target, label));
/** Raw text with its [[links]] made clickable and its ![[images]] shown. */
const inlineLinks = (text) => linkify(text, inlineLink);

/** Status words that end a quest or a life, or say it's under way, get their own badge and icon. */
const STATUS_ICONS = { done: "done", failed: "failed", dead: "failed", "in progress": "progress" };

/** A status or rarity as a badge, with an icon when the word has one. */
const badgeHtml = (key, value) =>
  `<span class="${key} ${key}-${escape(value.toLowerCase().replace(/[^a-z]/g, ""))}">${STATUS_ICONS[value.toLowerCase()] ? icon(STATUS_ICONS[value.toLowerCase()]) : ""}${escape(value)}</span>`;

/** A quest's status as a dropdown of QUEST_STATUSES (and whatever else the page says, so it isn't lost). */
function statusSelect(value) {
  const word = value.toLowerCase();
  const options = QUEST_STATUSES.includes(word) ? QUEST_STATUSES : [...QUEST_STATUSES, word];
  return `<select class="status status-${escape(word.replace(/[^a-z]/g, ""))}" data-prop="status" aria-label="Status">` +
    options.map((s) => `<option value="${escape(s)}"${s === word ? " selected" : ""}>${escape(s)}</option>`).join("") + "</select>";
}

/** Amounts of coin in a reward ("25gp", "3 sp"), each shown with a coin of its metal. */
const COIN = /\d[\d,]*\s*(pp|gp|ep|sp|cp)\b/gi;

/** "2026-10-04" as "4 Oct 2026"; anything else as written. */
const prettyDate = (v) =>
  /^\d{4}-\d{2}-\d{2}$/.test(v) ? new Date(`${v}T00:00`).toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" }) : null;

/** A note's author in a shared session: their name linking to their PC page, with its portrait small when it has one
 * (the DM without a character is plain "DM": see authorLink). */
function authorHtml(author) {
  if (!author) return "";
  if (author === DM) return authorLink(author); // never a PC page named DM, nor its portrait
  const page = pcPath(author, paths());
  const value = page ? splitFrontmatter(note(page).content).props.find(([k]) => k.toLowerCase() === "portrait")?.[1] ?? "" : "";
  const target = imageTarget(value);
  const pic = target && resolveImage(target, vault.images ?? []) ? `<span class="author-portrait">${imageHtml(target, "")}</span>` : "";
  return authorLink(author, page ? page.replace(/\.md$/i, "") : author, pic);
}

/** A page's properties, as a small stat block: see shownProps. */
function propsHtml(props, path) {
  const shown = shownProps(props, paths(), kindOf(path));
  const quest = kindOf(path) === "quest" && !isDmCopy(path);
  // A portrait ("[[Attachments/Demus.jpg]]" or a plain path) shows as a picture in the corner, not as a row.
  const pic = shown.rows.find((r) => r.key === "portrait");
  shown.rows = shown.rows.filter((r) => r !== pic);
  const portrait = pic ? `<div class="props-portrait">${imageHtml(imageTarget(pic.value), baseName(path))}</div>` : "";
  const rows = shown.rows.map(({ key, label, value, icon: name, path }) => {
    let html;
    if (key === "status" && quest) {
      html = statusSelect(value);
    } else if (key === "status" || key === "rarity") {
      html = badgeHtml(key, value);
    } else if (key === "dndbeyond") {
      // The sheet opens in the browser (the document click handler sends https links to open_url).
      html = sheetId(value)
        ? `<a href="${escape(value)}">Character sheet</a> <button type="button" class="ghost icon-button props-action" data-ddb-refresh ` +
          `title="Refresh from D&amp;D Beyond" aria-label="Refresh from D&amp;D Beyond">${icon("refresh")}</button>`
        : inlineLinks(value);
    } else if (key === "reward" && !value.includes("[[") && value.search(COIN) >= 0) {
      html = escape(value).replace(COIN, (m, c) => `<span class="coin coin-${c.toLowerCase()}">${icon("coin")}${m}</span>`);
    } else if (path) {
      html = `<a class="wikilink" data-target="${escape(value)}" href="#">${icon(name)}${escape(value)}</a>`;
    } else {
      const date = prettyDate(value);
      html = date ? `<time datetime="${escape(value)}">${escape(date)}</time>` : `${name ? icon(name) : ""}${inlineLinks(value)}`;
    }
    return `<dt>${escape(label)}</dt><dd>${html}</dd>`;
  });
  const sum = shown.summary ? `<p class="props-summary">${icon(kindOf(path))}${escape(shown.summary)}</p>` : "";
  return portrait || sum || rows.length ? `<div class="props">${portrait}${sum}${rows.length ? `<dl>${rows.join("")}</dl>` : ""}</div>` : "";
}

// ---------- sidebar ----------

function treeHtml(node, names = {}) {
  const dirs = node.dirs
    .map((d) => {
      const n = d.files.length;
      const count = n ? `<span class="sr-only">, </span>${n}<span class="sr-only"> page${n === 1 ? "" : "s"}</span>` : "";
      // A folder that's only in Private/ (shown under its own name, see sidebarTree) is that one: New page there starts private.
      const real = !vault.folders.includes(d.path) && vault.folders.includes(`Private/${d.path}`) ? `Private/${d.path}` : d.path;
      const empty = `<button type="button" class="folder-new" data-folder="${escape(real)}">+ New ${escape(typeFor(d.path) || "page")}</button>`;
      // A private-only folder, and each player's folder of private notes on a DM's computer: a padlock and who reads them.
      const lock = real !== d.path || names[d.path] ? lockHtml(`${real}/`) : "";
      return `<details data-folder="${escape(real)}"${closedFolders.has(real) ? "" : " open"}>
        <summary>${lock}${escape(names[d.path] ?? d.name)}<span class="count">${count}</span></summary>
        <div class="children">${treeHtml(d) || empty}</div></details>`;
    })
    .join("");
  const files = node.files
    .map((p) => {
      // Finished quests: dimmed, with a check or a cross, and the outcome in the accessible name.
      const status = p.startsWith("Quests/") ? questStatus(note(p)?.content ?? "") : "";
      const ended = status === "done" || status === "failed";
      // A private page, and a session that has only private notes so far: a padlock and who reads them (a DM's copies
      // have theirs on their player's folder).
      const only = note(p)?.parts?.length === 0 && note(p).private?.[0]?.path;
      const lock = isDmCopy(p) ? "" : isPrivate(p) ? lockHtml(p) : only ? lockHtml(only) : "";
      return `<button draggable="true" class="file${p === current ? " active" : ""}${ended ? " quest-ended" : ""}"${p === current ? ' aria-current="page"' : ""} data-path="${escape(p)}" title="${escape(p)}">` +
        `${lock}${ended ? icon(status) : ""}${escape(baseName(p))}${ended ? `<span class="sr-only">, ${status}</span>` : ""}</button>`;
    })
    .join("");
  return dirs + files;
}

function renderTree() {
  const scroll = $("tree").scrollTop;
  const all = paths();
  let html = treeHtml(sidebarTree(vault.folders, all.filter((p) => !isDmCopy(p))));
  // A DM's copies of the players' private notes: one group, a folder per player, read-only.
  const copies = all.filter(isDmCopy);
  const players = buildTree([], copies).dirs[0]?.dirs[0];
  if (players) {
    const names = Object.fromEntries(players.dirs.map((d) => [d.path, playerName(d.name)]));
    html += `<details data-folder="dm"${closedFolders.has("dm") ? "" : " open"}><summary>${icon("lock")}Players' private notes</summary>
      <div class="children">${treeHtml(players, names)}</div></details>`;
  }
  replaceHtml($("tree"), html);
  $("tree").scrollTop = scroll;
}

/** Where a page is, in words: "NPCs", "Private / NPCs", or "Syloth's private notes / NPCs" on a DM's computer. */
function whereOf(path) {
  const folder = unprivate(path).split("/").slice(0, -1).join(" / ");
  const head = isDmCopy(path) ? `${playerName(dmCopyOf(path))}'s private notes` : path.startsWith("Private/") ? "Private" : "";
  return [head, folder].filter(Boolean).join(" / ");
}

function renderResults() {
  const q = $("search").value;
  $("tree").hidden = !!q.trim();
  $("results").hidden = !q.trim();
  const count = (text) => { // unchanged text stays silent, so focus/vault refreshes don't re-announce
    if ($("result-count").textContent !== text) $("result-count").textContent = text;
  };
  if (!q.trim()) return count("");
  const mark = (s) => escape(s).replace(new RegExp(escape(q.trim()).replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "gi"), (m) => `<mark>${m}</mark>`);
  const hits = search(q, vault.notes);
  count(`${hits.length} result${hits.length === 1 ? "" : "s"}`);
  $("results").innerHTML = hits.length
    ? hits
        .map((h) => {
          const folder = whereOf(h.path);
          return `<button class="file" data-path="${escape(h.path)}">${isPrivate(h.path) ? lockHtml(h.path) : ""}${mark(baseName(h.path))}
            ${folder ? `<span class="where">${escape(folder)}</span>` : ""}
            ${h.snippet ? `<span class="snip">${mark(h.snippet.slice(0, 160))}</span>` : ""}</button>`;
        })
        .join("")
    : '<div class="empty">No matches</div>';
}

function selectResult(i) {
  const items = [...$("results").querySelectorAll(".file")];
  items.forEach((b, j) => b.classList.toggle("selected", j === i));
  if (items[i]) items[i].focus();
  else $("search").focus();
}

// ---------- page ----------

const legendHtml =
  `<dl class="prefix-legend">${NOTE_PREFIXES.map(({ keys: [k], kind }) => `<dt><kbd>${escape(k)}</kbd></dt><dd>${icon(kind)}${KINDS[kind]}</dd>`).join("")}</dl>`;

/** An empty session: the global shortcuts as set, the note symbols, and the way to the cheat sheet. */
const emptySessionHtml = (keys = globalKeys(settings)) => `<div class="empty-state">${icon("session")}<h2>No notes yet</h2>
  <p>Press <kbd>${escape(readable(keys.quickNote, mac))}</kbd> for a quick note, or <kbd>${escape(readable(keys.capture, mac))}</kbd> to save selected text (or the clipboard).</p>
  <p>Notes appear here live during the game. Start a note with a symbol to file it:</p>${legendHtml}
  <p class="legend-note">A <kbd>!</kbd> note goes in this session's Quests section. To follow a quest across sessions, make a Quest page with New page: open ones are listed at the top of every session.</p>
  <p class="home-actions"><button type="button" class="button-link" data-action="cheatSheet">Cheat sheet</button></p></div>`;

/**
 * The notes in order, with their authors in a shared session; `removable(note)` adds that row's delete button (the
 * session's own Timeline view: every note, or in a shared session only yours).
 */
const timelineHtml = (items, removable = () => false) =>
  `<ol class="timeline">${items
    .map((item) => {
      const { time, shown, kind, text, line, author, path } = item;
      const badge = KINDS[kind] ? `<span class="kind kind-${kind}">${icon(kind)}${KINDS[kind]}</span>` : "";
      const remove = removable(item)
        ? `<button type="button" class="remove-note" data-line="${line}" data-path="${escape(path ?? current)}" aria-label="Delete note: ${escape(stripLinks(text))}" title="Delete note">${icon("trash")}</button>`
        : "";
      const lock = path && isPrivate(path) ? lockHtml(path) : "";
      return `<li><time>${escape(shown ?? time)}</time>${badge}<span class="text">${inlineLinks(text)}${author ? ` <span class="by">${authorHtml(author)}</span>` : ""}${lock}</span>${remove}</li>`;
    })
    .join("")}</ol>`;

/** The Quests/ pages still open, at the top of every session; nothing when there are none. */
function openQuestsHtml() {
  const quests = openQuests(vault.notes);
  if (!quests.length) return "";
  const items = quests.map((p) => `<li><a data-path="${escape(p)}" href="#">${icon("quest")}${escape(baseName(p))}</a></li>`);
  return `<section class="open-quests" aria-labelledby="open-quests-title"><h2 id="open-quests-title">Open quests</h2><ul>${items.join("")}</ul></section>`;
}

/** A session page `n`: a shared one merges every player's file (see mergeTimelines), and only your own notes can be deleted. */
function sessionHtml(n) {
  const session = n.parts ? parseMerged([...n.parts, ...(n.private ?? [])], paths()) : parse(n.content);
  const items = sessionItems(n);
  const title = `<h1 class="session-title">${session.title ? inlineLinks(session.title) : escape(baseName(current))}</h1>`;
  if (!items.length) return title + openQuestsHtml() + emptySessionHtml();
  const pressed = (v) => `aria-pressed="${sessionView === v}"`;
  const views = `<div class="view-switch" role="group" aria-label="Session view">
    <button type="button" data-view="timeline" ${pressed("timeline")}>Timeline</button>
    <button type="button" data-view="journal" ${pressed("journal")}>Journal</button></div>`;
  const mine = [myFile(current), myPrivateFile(current)];
  return views + title + openQuestsHtml() + (sessionView === "journal"
    ? `<div class="journal">${toHtml({ ...session, title: "" }, inlineLink, (item) => lockHtml(item.path))}</div>` +
      `<p class="session-note">A recap of the session, grouped by kind. Click Edit to see every line.</p>`
    : timelineHtml(items, n.parts ? (item) => mine.includes(item.path) : () => true));
}

/** Whether page `path` shows the private notes that link it: a shared page, not a session, in a shared campaign. */
const showsMentions = (path) => !!privateLabel(sharing()) && !isPrivate(path) && !isSession(path);

/** A line of a private note that links the open page, with a padlock and its source ("Session 1, 20:14", a private page), which opens it. */
function mentionHtml(src, line) {
  const [, time = "", rest] = line.match(/^(?:[-*+] )?(?:(\d{1,2}:\d{2}) )?(.*)$/);
  const session = unprivate(src).match(/^Sessions\/Session \d+/i)?.[0];
  const where = session ? `${baseName(session)}${time ? `, ${localTime(time, note(src)?.content ?? "")}` : ""}` : baseName(src); // in your zone
  const target = (session && [session, `${session}.md`].find(note)) || src; // the session's page, else the note itself
  const label = isDmCopy(src) ? `${playerName(dmCopyOf(src))}, ${where}` : where;
  return `<li>${lockHtml(src)}<a data-path="${escape(target)}" href="#">${escape(label)}</a>: ${inlineLinks(rest.replace(/^[@#!?]\s*/, ""))}</li>`;
}

/**
 * The private notes that link shared page `path`, at its bottom, for you only (and the DM, as the label says): rendered
 * from their own files, never merged into the page's content, which the map, backlinks and search read. Nothing when none do.
 */
function mentionsHtml(path) {
  if (!showsMentions(path)) return "";
  const items = privateMentions(path, vault.notes).flatMap((m) => m.lines.map((l) => mentionHtml(m.path, l)));
  return items.length ? `<section class="private-mentions" aria-labelledby="private-mentions-title">
    <h2 id="private-mentions-title"><span class="lock" aria-hidden="true">${icon("lock")}</span>Private notes</h2>
    <p class="private-mentions-who">${escape(ownLabel())}</p><ul>${items.join("")}</ul></section>` : "";
}

// ---------- home ----------

/** Before there's a campaign: make one, or join one a friend shares (the Join dialog is in Settings). */
const firstRunHtml = `<div class="empty-state first-run">${icon("home")}<h1>Welcome to Lorekeeper</h1>
  <p>Each campaign keeps its notes in a folder of its own, in the Lorekeeper folder in Documents. Start with one.</p>
  <form id="first-run-form" class="first-run-form" novalidate>
    <label>Campaign name <input id="first-run-name" autocomplete="off" spellcheck="false" placeholder="Curse of Strahd" /></label>
    <button class="seal">Create campaign</button>
  </form>
  <p id="first-run-error" class="first-run-error" role="alert"></p>
  <p>Playing in a campaign someone shared with you?</p>
  <p><button type="button" class="ghost" data-action="joinCampaign">Join a shared campaign…</button></p></div>`;

/** Hotkeys and buttons that make pages need a campaign: false (and says so) before there is one. */
function haveCampaign() {
  if (settings.vaultPath) return true;
  say("Create or join a campaign first");
  return false;
}

/** Puts a zoomed or moved map back to showing everything (graph-view.js); the same markup is in index.html. */
const FIT_BUTTON = `<button type="button" class="ghost icon-button graph-fit" aria-label="Fit" title="Fit the whole map (or double-click it)" disabled>${icon("fit")}</button>`;

/** The campaign's contents page: latest session, open quests, the party, who and what came up lately, the sessions. */
function homeHtml(graph) {
  const list = sessions(vault.notes);
  const latest = list.find((s) => s.path === vault.currentSession) ?? list[0]; // where hotkey notes go
  const quests = openQuests(vault.notes);
  const pcs = party(vault.notes);
  const seen = recentlyMentioned(vault.notes);
  const plural = (n, word) => `${n} ${word}${n === 1 ? "" : "s"}`;
  const dated = (s, count) => `${s.date && !s.title.includes(s.date) ? `${escape(s.date)} · ` : ""}${plural(count, "note")}`; // "Session 3 - 2026-10-04" says it already
  const link = (path, iconName = "") => `<a href="#" data-path="${escape(path)}">${iconName && icon(iconName)}${escape(baseName(path))}</a>`;
  const card = (id, title, iconName, body, wide = false) =>
    `<section class="home-card${wide ? " wide" : ""}" aria-labelledby="home-${id}"><h2 id="home-${id}">${icon(iconName)}${title}</h2>${body}</section>`;
  const cards = [];
  if (vault.conflicts.length) {
    // A copy that's a note opens like any page; a folder or an image is just named.
    const name = (path) => (note(path) ? link(path) : escape(path));
    const items = vault.conflicts.map((c) => `<li>${name(c.path)}${c.of ? ` <span class="home-meta">copy of ${name(c.of)}</span>` : ""}</li>`);
    cards.push(card("conflicts", "Sync conflicts", "mystery", `
      <p class="home-meta">These were changed on two computers at once, so sync kept both versions. Open both, keep what you want in the original, then delete the copy.</p>
      <ul class="home-list">${items.join("")}</ul>`, true));
  }
  if (latest) {
    const items = sessionItems(note(latest.path));
    cards.push(card("latest", "Latest session", "session", `
      <h3 class="home-latest"><a href="#" data-path="${escape(latest.path)}">${escape(latest.title)}</a></h3>
      <p class="home-meta">${dated(latest, items.length)}</p>
      ${items.length ? timelineHtml(items.slice(-5)) : `<p class="home-none">No notes yet. Press <kbd>${escape(readable(globalKeys(settings).quickNote, mac))}</kbd> during the game to jot one.</p>`}`, true));
  }
  if (quests.length) cards.push(card("quests", "Open quests", "quest", `<ul class="home-list">${quests.map((p) => `<li>${link(p, "quest")} ${badgeHtml("status", questStatus(note(p).content))}</li>`).join("")}</ul>`));
  if (pcs.length) {
    const who = (pc) => {
      const what = [pc.race, pc.class].filter(Boolean).join(" ");
      const level = pc.level ? `level ${pc.level}` : "";
      const details = [[what, level].filter(Boolean).join(", "), pc.player && `played by ${pc.player}`].filter(Boolean).join(" · ");
      return `<li>${link(pc.path, "pc")}${details ? ` <span class="home-meta">${inlineLinks(details)}</span>` : ""}</li>`;
    };
    cards.push(card("party", "The party", "pc", `<ul class="home-list">${pcs.map(who).join("")}</ul>`));
  }
  if (seen.length) {
    const items = seen.map((m) => `<li>${link(m.path, m.kind)} <span class="home-meta">${plural(m.count, "mention")}</span></li>`);
    cards.push(card("mentioned", "Recently mentioned", "npc", `<ul class="home-list">${items.join("")}</ul>`));
  }
  if (list.length) {
    const items = list.slice(0, 5).map((s) =>
      `<li><a href="#" data-path="${escape(s.path)}">${escape(s.title)}</a><span class="leader" aria-hidden="true"></span>` +
      `<span class="home-meta">${dated(s, s.count)}</span></li>`);
    cards.push(card("sessions", "Sessions", "note", `<ul class="home-list contents">${items.join("")}</ul>`));
  }
  if (graph.edges.length) {
    cards.push(card("map", "Connections", "faction", `<div class="graph-box"><canvas class="graph" role="img"
      aria-label="Map of ${graph.nodes.length} people, places and factions and who links to whom"></canvas>${FIT_BUTTON}</div>
      <p class="home-meta">Scroll or pinch to zoom, drag to move, double-click to see it all. Pages show their portrait or first picture.</p>`, true));
  }
  if (!cards.length) {
    return `<div class="empty-state">${icon("home")}<h1>Welcome to Lorekeeper</h1>
      <p>This page gathers your campaign at a glance: the latest session, open quests, the party and who you've met.</p>
      <p>Start a session, then press <kbd>${escape(readable(globalKeys(settings).quickNote, mac))}</kbd> during the game to jot a note.</p>
      <p class="home-actions"><button type="button" class="seal" data-action="newSession">+ New session</button>
      <button type="button" class="ghost" data-action="newPage">+ New page</button></p></div>`;
  }
  const campaign = campaignName(settings.vaultPath);
  return `<h1 class="home-title">${escape(campaign)}</h1><div class="home-grid">${cards.join("")}</div>`;
}

/**
 * Replaces `el`'s HTML, keeping focus on the same link or button (found by its data attributes) when it's still there,
 * since refreshes now also come from a teammate's notes arriving.
 */
function replaceHtml(el, html) {
  const was = el.contains(document.activeElement) ? document.activeElement : null;
  el.innerHTML = html;
  const keys = Object.entries(was?.dataset ?? {});
  if (!keys.length) return;
  const selector = was.tagName + keys.map(([k, v]) => `[data-${k.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`)}="${CSS.escape(v)}"]`).join("");
  el.querySelector(selector)?.focus({ preventScroll: true });
}

let shown = ""; // the page HTML on screen: re-rendering the same HTML would only lose keyboard focus
function setView(html) {
  if (html !== shown) replaceHtml($("view"), (shown = html));
}

function render() {
  const n = note(current); // none: Home
  const folder = current ? current.split("/").slice(0, -1).join(" / ") : "";
  const where = current ? whereOf(current) : "";
  const label = current && isPrivate(current) ? `<span class="lock" aria-hidden="true">${icon("lock")}</span><span class="private-label">${escape(lockLabel(current))}${isDmCopy(current) ? ", read-only" : ""}</span>` : "";
  $("crumbs").innerHTML = n
    ? `<a href="#" data-home>Home</a><span class="folder"> / ${where ? `${escape(where)} / ` : ""}</span><strong>${escape(baseName(current))}</strong>${label}`
    : `<strong aria-current="page">Home</strong>`;
  $("home").classList.toggle("active", !n);
  n ? $("home").removeAttribute("aria-current") : $("home").setAttribute("aria-current", "page");
  $("back").disabled = nav.find(-1, alive, current) < 0;
  $("forward").disabled = nav.find(1, alive, current) < 0;
  const copy = !!current && isDmCopy(current); // a player's private note on a DM's computer: read-only
  $("delete").disabled = !n || (!!n.parts && !ownSession(current).length) || copy; // a shared session is everyone's notes, unless it's all yours
  $("delete").hidden = !n; // Home and the welcome page have no page to act on
  showPrivacy(n);
  $("backlinks").hidden = !n;
  $("connections").hidden = true; // until it has pages to show
  $("toggle").hidden = !n || copy;
  $("toggle").innerHTML = icon(editing ? "done" : "edit");
  $("toggle").setAttribute("aria-label", editing ? "Done" : "Edit");
  $("toggle").title = `${editing ? "Done" : "Edit"} (${readable("CmdOrCtrl+E", mac)})`;
  $("editor").hidden = !editing || !n;
  $("editor-hint").hidden = !editing || !n || !isSession(current);
  // Session notes | My notes | Private notes, when a shared session has more than one file you can edit.
  const choices = editing && n?.parts ? choicesFor(current) : [];
  $("edit-switch").hidden = choices.length < 2;
  if (choices.length > 1) {
    const picked = editTarget(choices, editChoice);
    const label = { session: "Session notes", mine: "My notes", private: `<span class="lock" aria-hidden="true">${icon("lock")}</span>Private notes` };
    $("edit-switch").innerHTML = `<div class="view-switch" role="group" aria-label="Notes to edit">${choices
      .map((c) => `<button type="button" data-choice="${c.id}" aria-pressed="${c.path === picked}">${label[c.id]}</button>`).join("")}</div>${
      isPrivate(picked) ? `<span class="private-label">${escape(ownLabel())}</span>` : ""}`;
  }
  $("view").hidden = editing && !!n;

  $("new-page").disabled = $("new-session").disabled = !settings.vaultPath;
  if (startView(settings) === "first-run") return setView(firstRunHtml);
  const graph = !n || GRAPH_KINDS.includes(kindOf(current)) ? connections(vault.notes, vault.images ?? []) : null;
  if (!n) {
    setView(homeHtml(graph));
    const canvas = $("view").querySelector("canvas.graph");
    if (canvas) drawGraph(canvas, graph, { onOpen: open });
    return;
  }
  if (!editing) {
    const { props, body } = splitFrontmatter(n.content);
    setView(isSession(current) ? sessionHtml(n) : propsHtml(props, current) + marked.parse(body) + mentionsHtml(current));
  }
  const near = graph && neighbourhood(graph, current);
  if (near?.nodes.length > 1) {
    const others = near.nodes.filter((p) => p.path !== current).map((p) => baseName(p.path));
    const canvas = $("connections").querySelector("canvas");
    $("connections").hidden = false;
    canvas.setAttribute("aria-label", `${baseName(current)} is linked with ${others.join(", ")}`);
    drawGraph(canvas, near, { focus: current, onOpen: open });
  }
  // Private notes that link a page showing them are listed there instead (mentionsHtml).
  const links = backlinks(current, showsMentions(current) ? vault.notes.filter((b) => !isPrivate(b.path)) : vault.notes);
  $("backlinks").innerHTML = `<h2>Linked from</h2>` + (links.length
    ? `<ul>${links
        .map((b) => `<li><a data-path="${escape(b.path)}" href="#">${escape(baseName(b.path))}</a>
          ${b.lines.map((l) => `<div class="snip">${inlineLinks(l.replace(/^[-*+] (\d{1,2}:\d{2} )?/, ""))}</div>`).join("")}</li>`)
        .join("")}</ul>`
    : `<p class="none">No pages link here yet. Link to it with [[${escape(baseName(current))}]].</p>`);
}

/** Shows a page (null: Home), saving pending edits first. A new visit goes into history; `to` is Back / Forward's index. */
async function open(path, { edit = false, to } = {}) {
  await flush();
  if (held) return; // ~ lines that couldn't be moved stay in the editor; save says why
  if (to === undefined) nav.visit(path);
  else nav.go(to);
  current = path;
  editChoice = null;
  base = note(editPath())?.content ?? "";
  editing = edit;
  inConflict = false;
  $("conflict").hidden = true;
  if (editing) ed().setValue(base, { reset: true });
  renderTree();
  render();
  $("scroll").scrollTop = 0;
  if (editing) ed().focus();
}

/** Back (-1) or Forward (1), skipping pages deleted or renamed since. */
function go(dir) {
  const i = nav.find(dir, alive, current);
  if (i >= 0) open(nav.entry(i), { to: i });
}

function followLink(target) {
  const path = resolve(target, paths());
  if (path) open(path);
  else if (!isImage(target)) openNewDialog(target.split("/").pop()); // not a page to make
}

// ---------- saving ----------

function scheduleSave() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    saveTimer = null;
    saving = save().finally(() => (saving = null));
  }, 600);
}

/** Saves pending typing now, a ~ line still being typed included (see save). */
async function flush() {
  await saving;
  if (saveTimer || held) {
    clearTimeout(saveTimer);
    saveTimer = null;
    saving = save(true).finally(() => (saving = null));
  }
  await saving;
}

/** The session number of shared session file `path` ("Sessions/Session 4/Sibling 5.md" or "Sessions/Session 4.md"), else null. */
const sessionNumber = (path) => Number(path.match(/^Sessions\/Session (\d+)(?:\.md$|\/)/i)?.[1]) || null;

/**
 * The `~` lines in the editor's text for shared file `path` (splitPrivate), in a shared campaign once you picked a
 * character; null otherwise, and in your private files, where a `~` stays as typed (as in a campaign of your own).
 */
const privateLines = (path, content) =>
  me() && !isPrivate(path) ? splitPrivate(content, sessionNumber(path) ? "" : current, paths()) : null;

/**
 * Saves the editor's text. Lines typed with a ~ go to your private notes first, then leave the editor: a shared file
 * never gets them, even briefly. While the cursor is still on one, nothing is saved (`held`, so a pause mid-line never
 * sends half of it and leaves the rest to be typed into the shared page), until the cursor leaves it or `final`
 * (flush: Done, another page, quitting).
 */
async function save(final = false) {
  const path = editPath();
  let content = ed().getValue();
  held = !final && !!privateLines(path, ed().cursorLine())?.private.length;
  if (held) return;
  let moved = 0;
  for (let split; (split = privateLines(path, content))?.private.length; content = ed().getValue()) {
    try {
      await invoke("save_private_lines", { session: sessionNumber(path), lines: split.private });
    } catch (err) {
      held = true; // nothing is saved, and the editor stays open with it (open, Done)
      return say(`Not saved: couldn't move your ~ notes to your private notes: ${err}`);
    }
    moved += split.private.length;
    // Typing during the write stays; the loop then checks it for ~ lines too.
    const now = ed().getValue(), left = now.split("\n");
    for (const line of now === content ? [] : split.taken) if (left.includes(line)) left.splice(left.indexOf(line), 1);
    ed().setValue(now === content ? split.shared : left.join("\n"));
  }
  try {
    const written = await invoke("save_file", { path, content, base });
    base = written;
    const n = note(path);
    if (n) n.content = written;
    if (path.startsWith("Quests/")) renderTree(); // a changed status moves the check mark
    // Notes added by the hotkeys while editing were kept on disk; show them in the editor too.
    if (written !== content && written.startsWith(content) && path === editPath()) ed().append(written.slice(content.length));
    say(moved ? `Moved ${moved} note${moved === 1 ? "" : "s"} to your private notes (${ownLabel()})` : "Saved");
  } catch (err) {
    if (err === "conflict") {
      inConflict = true;
      $("conflict").hidden = false;
    } else say(`Not saved: ${err}`);
  }
}

const MAX_IMAGE = 20 * 1024 * 1024; // lib.rs refuses bigger ones too

/**
 * Saves a pasted or dropped image into Attachments/ and returns the name to embed, or null when it wasn't saved.
 * Pasted images get Obsidian's "Pasted image <time>" name, dropped ones keep theirs; either way a name no other image
 * in the vault has, so ![[name]] finds this one.
 */
async function saveImage(file, pasted) {
  if (file.size > MAX_IMAGE) {
    say("Not saved: images can be at most 20 MB");
    return null;
  }
  const own = safeName(file.name ?? "");
  const wanted = !pasted && isImage(own) && !own.startsWith(".") ? own : pastedName(new Date(), file.type);
  if (!wanted) {
    say("Not saved: only PNG, JPG, GIF, WebP and SVG images");
    return null;
  }
  const data = await new Promise((ok, fail) => {
    const r = new FileReader();
    r.onload = () => ok(r.result.slice(r.result.indexOf(",") + 1)); // base64 after "data:image/png;base64,"
    r.onerror = () => fail(r.error);
    r.readAsDataURL(file);
  });
  for (;;) {
    const name = freeName(wanted, vault.images ?? []);
    const path = `${ATTACHMENTS}/${name}`;
    let err = null;
    await invoke("save_image", { path, data }).catch((e) => (err = String(e)));
    if (err && !err.endsWith("already exists.")) {
      say(`Not saved: ${err}`);
      return null;
    }
    vault.images = [...(vault.images ?? []), path]; // saved now, or made outside the app since the vault was read
    if (!err) {
      say(`Saved ${path}`);
      return name;
    }
  }
}

async function refresh() {
  try {
    await loadVault();
  } catch (err) {
    return say(`Couldn't read notes: ${err}`);
  }
  if (current && !note(current)) current = null; // deleted or renamed elsewhere
  if (current) current = pageFor(current); // a session that became shared since
  const n = note(editPath());
  if (n && !dirty() && !inConflict) {
    if (editing) ed().setValue(n.content); // keeps the cursor
    base = n.content;
  }
  renderTree();
  renderResults();
  render();
}

// ---------- new page ----------

const FOLDERS = { NPC: "NPCs", PC: "PCs", Location: "Locations", Item: "Items", Faction: "Factions", Quest: "Quests", Lore: "Lore" };
const TYPES = Object.keys(FOLDERS); // built in: the vault gets their templates and folders
let lastType = "Note"; // what "+ New page" offers first: the type last created, until restart
let newFrom = ""; // the folder the dialog was opened from, where a Note goes

/** What "New page" can make: the built-in types, your own templates in Templates/, then Note (blank, or Templates/Note.md). */
function newTypes() {
  const known = [...TYPES, "Note"].map((t) => t.toLowerCase());
  const own = [...new Set(templates.map((t) => baseName(t.path)))].filter((t) => !known.includes(t.toLowerCase()));
  return [...TYPES, ...own.sort((a, b) => a.localeCompare(b)), "Note"];
}

/** The type whose pages live in `folder` ("NPCs" holds NPC pages), or "". */
const typeFor = (folder) => newTypes().find((t) => t !== "Note" && folderFor(t, [folder])) ?? "";

/** Where a new page goes: its type's folder (created for a built-in type), else the vault root; a Note goes where the dialog was opened. */
const folderOf = (type) => (type === "Note" ? newFrom : folderFor(type, vault.folders) || (FOLDERS[type] ?? ""));

const templateOf = (type) => templates.find((t) => baseName(t.path).toLowerCase() === type.toLowerCase());
const chosenType = () => $("new-chips").querySelector("input:checked")?.value ?? "Note";
function syncNewTitle() {
  $("new-title").textContent = chosenType() === "Note" ? "New note" : `New ${chosenType()}`;
  $("new-ddb").hidden = chosenType() !== "PC";
}

/**
 * Asks what you're making and its name; the type picks the template and the folder. From a folder (its empty-folder
 * button, "New Page Here") that folder's type is preselected; from a broken link the name is filled in and focus starts
 * on the type.
 */
function openNewDialog(name = "", { folder = "" } = {}) {
  // From a folder in Private/ the page is private (the switch starts on); a DM's copies take no new pages.
  if (isDmCopy(`${folder}/`) || folder === "dm") folder = "";
  const startPrivate = folder === "Private" || folder.startsWith("Private/");
  folder = startPrivate ? folder.replace(/^Private\/?/, "") : folder;
  newFrom = folder;
  const label = privateLabel(sharing());
  $("new-private-row").hidden = !label; // only in a campaign shared with your party
  $("new-private-text").textContent = label;
  $("new-private").checked = !!label && startPrivate;
  const types = newTypes();
  const pick = folder ? typeFor(folder) || "Note" : types.includes(lastType) ? lastType : "Note";
  $("new-chips").innerHTML = types
    .map((t) => `<label class="chip"><input type="radio" name="new-type" value="${escape(t)}"${t === pick ? " checked" : ""} />` +
      `${icon(TYPES.includes(t) ? t.toLowerCase() : "note")}<span>${escape(t)}</span></label>`)
    .join("");
  syncNewTitle();
  $("new-name").value = name;
  $("new-error").textContent = "";
  $("new-dialog").showModal();
  (name ? $("new-chips").querySelector("input:checked") : $("new-name")).focus();
}

$("new-chips").addEventListener("change", syncNewTitle);
// Arrow keys move between the chips natively (they're radios); Enter creates from there too, as it does from the name.
$("new-chips").addEventListener("keydown", (e) => {
  if (e.key !== "Enter") return;
  e.preventDefault();
  $("new-form").requestSubmit($("new-create"));
});
$("new-cancel").addEventListener("click", () => $("new-dialog").close());

$("new-form").addEventListener("submit", async (e) => {
  if (e.submitter?.value !== "create") return;
  e.preventDefault();
  const name = $("new-name").value.trim();
  const problem = badName(name);
  if (problem) {
    $("new-error").textContent = problem;
    return $("new-name").focus();
  }
  const type = chosenType();
  const folder = folderOf(type);
  const shared = folder ? `${folder}/${name}.md` : `${name}.md`;
  const path = !$("new-private-row").hidden && $("new-private").checked ? `Private/${shared}` : shared;
  const tpl = templateOf(type);
  const content = tpl ? fillTemplate(tpl.content, name, today()) : `# ${name}\n\n`;
  try {
    await invoke("create_file", { path, content }); // makes the folder when it's missing
  } catch (err) {
    return ($("new-error").textContent = err);
  }
  lastType = type;
  $("new-dialog").close();
  await refresh();
  record({ label: `Create ${name}`, undo: () => trashIfUnchanged(path, content), redo: () => restore(path, content) });
  open(path, { edit: true });
});

// ---------- D&D Beyond: PC pages from characters (dndbeyond.rs fetches them) ----------

let found = []; // the characters the last lookup returned
let lookups = 0; // a lookup answering after the dialog was reopened or another started is dropped

function openDdbDialog() {
  found = [];
  lookups++;
  $("ddb-find").disabled = false;
  $("ddb-find").textContent = "Look up";
  $("ddb-link").value = "";
  $("ddb-error").textContent = "";
  renderFound();
  $("ddb-dialog").showModal();
  $("ddb-link").focus();
}

/** The checklist: each character with what it is, who plays it and whether it updates a page or makes one. */
function renderFound() {
  $("ddb-found").hidden = !found.length;
  $("ddb-list").innerHTML = found
    .map((c, i) => {
      const page = c.error ? null : pcPageFor(c, vault.notes);
      const what = [[c.race, c.classes].filter(Boolean).join(" "), c.level ? `level ${c.level}` : ""].filter(Boolean).join(", ");
      const details = c.error || [what, c.player && `played by ${c.player}`, page ? `update ${baseName(page)}` : "new page"].filter(Boolean).join(" · ");
      return `<label class="ddb-row"><input type="checkbox" value="${i}"${c.error ? " disabled" : " checked"} />` +
        `<span><strong>${escape(c.name)}</strong> <span class="home-meta">${escape(details)}</span></span></label>`;
    })
    .join("");
  syncImport();
}
const syncImport = () => ($("ddb-import").disabled = !$("ddb-list").querySelector("input:checked"));

async function lookUp() {
  const link = $("ddb-link").value.trim();
  $("ddb-error").textContent = "";
  if (!link) {
    $("ddb-error").textContent = "Paste a character or campaign link first.";
    return $("ddb-link").focus();
  }
  $("ddb-find").disabled = true;
  $("ddb-find").textContent = "Looking up…";
  const attempt = ++lookups;
  let result = [];
  try {
    result = await invoke("dndbeyond_lookup", { link });
  } catch (err) {
    if (attempt === lookups) $("ddb-error").textContent = String(err);
  }
  if (attempt !== lookups) return;
  found = result;
  $("ddb-find").disabled = false;
  $("ddb-find").textContent = "Look up";
  renderFound();
  ($("ddb-list").querySelector("input:checked") ?? $("ddb-link")).focus();
}

/** Puts a page's text back to `content` while it still holds `expected` (the import's own undo and redo). */
async function putBack(path, expected, content) {
  await flush();
  if (note(path)?.content !== expected) throw `${baseName(path)} has changed since, so it was kept`;
  await rewrite(path, () => content);
}

/** Undoes (back) or redoes an import's [path, before, after] changes ("" = no page); a page changed since is kept. */
async function replayImport(changes, back) {
  const kept = [];
  for (const [path, before, after] of changes) {
    const [from, to] = back ? [after, before] : [before, after];
    await (from ? (to ? putBack(path, from, to) : trashIfUnchanged(path, from)) : restore(path, to)).catch((err) => kept.push(String(err)));
  }
  if (kept.length) throw kept.join("; ");
}

/**
 * Saves a character's D&D Beyond portrait as Attachments/<name>.<ext> when the page has none yet, and returns its path
 * ("" when there's nothing to save or it failed: a missing portrait never stops an import).
 * ponytail: undoing an import leaves a downloaded portrait in Attachments/; remove it by hand if you want.
 */
async function savePortrait(c, md, name) {
  const has = splitFrontmatter(md).props.some(([k, v]) => k.toLowerCase() === "portrait" && v.replace(/^["']|["']$/g, "").trim());
  if (!c.portrait || has) return "";
  return invoke("dndbeyond_portrait", { url: c.portrait, stem: `${ATTACHMENTS}/${name}` }).catch(() => "");
}

/**
 * Writes characters into their PC pages (see pcPageFor and characterProps), making the ones that are missing from the PC
 * template, as one undoable action. Returns the pages written and the characters that failed.
 */
async function importCharacters(chars) {
  await flush();
  await loadVault(); // write over what's on disk now
  const changes = [];
  const failed = [];
  for (const c of chars) {
    const name = safePageName(c.name, `Character ${c.id}`);
    try {
      const page = pcPageFor(c, vault.notes);
      if (page) {
        const before = note(page).content;
        const portrait = await savePortrait(c, before, name);
        await rewrite(page, (md) => characterProps(md, c, portrait));
        if (note(page).content !== before) changes.push([page, before, note(page).content]);
        continue;
      }
      const folder = folderOf("PC");
      const taken = (p) => vault.notes.some((n) => n.path.toLowerCase() === p.toLowerCase());
      let path = `${folder}/${name}.md`;
      if (taken(path)) path = `${folder}/${name} (${c.id}).md`; // a page of that name belongs to another character
      const tpl = templateOf("PC");
      const content = characterProps(tpl ? fillTemplate(tpl.content, name, today()) : `# ${name}\n\n`, c, await savePortrait(c, "", name));
      await invoke("create_file", { path, content });
      vault.notes.push({ path, content });
      changes.push([path, "", content]);
    } catch (err) {
      failed.push(`${name}: ${err}`);
    }
  }
  await refresh();
  if (changes.length) record({ label: "Import from D&D Beyond", undo: () => replayImport(changes, true), redo: () => replayImport(changes, false) });
  return { changes, failed };
}

async function importChosen() {
  const chosen = [...$("ddb-list").querySelectorAll("input:checked")].map((box) => found[+box.value]);
  $("ddb-import").disabled = true;
  const { changes, failed } = await importCharacters(chosen);
  const done = `${changes.length} page${changes.length === 1 ? "" : "s"} written`;
  if (failed.length) {
    renderFound();
    $("ddb-error").textContent = `${done}. Couldn't import ${failed.join("; ")}`;
    return;
  }
  $("ddb-dialog").close();
  say(changes.length ? `Imported from D&D Beyond: ${done}` : "Already up to date");
  if (chosen.length === 1 && changes.length) open(changes[0][0]);
}

$("new-ddb").addEventListener("click", () => {
  $("new-dialog").close();
  openDdbDialog();
});
$("ddb-list").addEventListener("change", syncImport);
$("ddb-cancel").addEventListener("click", () => $("ddb-dialog").close());
$("ddb-form").addEventListener("submit", (e) => {
  const what = e.submitter?.value;
  if (what !== "find" && what !== "import") return;
  e.preventDefault();
  if (what === "find") lookUp();
  else importChosen();
});

/** "Refresh from D&D Beyond" on a PC page: the same update as an import, undoable. */
async function refreshCharacter(path, button) {
  const id = dndBeyondId(note(path)?.content ?? "");
  if (!id) return;
  const name = baseName(path);
  button.disabled = true;
  say(`Refreshing ${name}…`);
  try {
    const c = await invoke("dndbeyond_character", { id: Number(id) });
    const before = note(path).content;
    const portrait = await savePortrait(c, before, name);
    await rewrite(path, (md) => characterProps(md, c, portrait));
    if (portrait) await refresh(); // the new picture has to be in the vault's image list to show
    const after = note(path).content;
    if (after === before) return say(`${name} is up to date`);
    record({ label: `Refresh ${name}`, undo: () => putBack(path, after, before), redo: () => putBack(path, before, after) });
    say(`Updated ${name}${c.level ? `: level ${c.level}` : ""}`);
  } catch (err) {
    say(`Couldn't refresh ${name}: ${err}`);
  } finally {
    button.disabled = false; // a no-op once the page re-rendered
  }
}

// ---------- events ----------

document.addEventListener("click", (e) => {
  const el = e.target.closest("a, .file");
  if (!el) return;
  e.preventDefault();
  if (el.dataset.home !== undefined) return open(null);
  if (el.dataset.path) return open(el.dataset.path);
  if (el.dataset.target !== undefined) return followLink(el.dataset.target);
  const href = el.getAttribute("href") ?? "";
  if (/^https?:/i.test(href)) invoke("open_url", { url: href }).catch(say);
  else if (href && !href.startsWith("#") && !/^[a-z][a-z0-9+.-]*:/i.test(href)) followLink(decodeURIComponent(href));
});

// A file dropped outside the editor would replace the app in the window; the editor saves dropped images itself.
for (const type of ["dragover", "drop"]) document.addEventListener(type, (e) => e.dataTransfer?.types.includes("Files") && e.preventDefault());

$("tree").addEventListener("click", (e) => {
  const b = e.target.closest(".folder-new");
  if (b) openNewDialog("", { folder: b.dataset.folder });
});

// A quest's status dropdown writes the page's `status` property, with Undo.
$("view").addEventListener("change", async (e) => {
  const select = e.target.closest("select[data-prop]");
  if (!select) return;
  const path = current, before = note(path).content;
  try {
    await rewrite(path, (md) => setProps(md, { [select.dataset.prop]: select.value }));
  } catch (err) {
    return say(`Couldn't change the status: ${err}`);
  }
  const after = note(path).content;
  record({ label: "Change status", undo: () => putBack(path, after, before), redo: () => putBack(path, before, after) });
  renderTree(); // a finished quest gets its check mark
});

$("view").addEventListener("click", async (e) => {
  const button = e.target.closest("button");
  if (button?.dataset.action) return actions[button.dataset.action]();
  if (button?.dataset.ddbRefresh !== undefined) return refreshCharacter(current, button);
  if (button?.classList.contains("remove-note")) {
    const row = [...$("view").querySelectorAll(".remove-note")].indexOf(button);
    await deleteNote(button.dataset.path, +button.dataset.line);
    // Keyboard users stay in the list: the next note's button, else the previous one, else the toast's Undo.
    const left = $("view").querySelectorAll(".remove-note");
    (left[Math.min(row, left.length - 1)] ?? ($("toast").hidden ? null : $("toast-undo")))?.focus();
    return;
  }
  const view = e.target.closest(".view-switch button")?.dataset.view;
  if (!view) return;
  sessionView = view;
  render();
  $("view").querySelector(`[data-view="${view}"]`)?.focus(); // re-render replaced the button
});

$("tree").addEventListener("toggle", (e) => {
  const path = e.target.dataset?.folder;
  if (path !== undefined) e.target.open ? closedFolders.delete(path) : closedFolders.add(path);
}, true);

$("search").addEventListener("input", renderResults);
$("search").addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    $("search").value = "";
    renderResults();
  }
  if (e.key === "Enter") $("results").querySelector(".file")?.click();
  if (e.key === "ArrowDown") {
    e.preventDefault();
    selectResult(0);
  }
});
// Arrow keys move between results (Enter opens the focused one natively); Escape goes back to the search box.
$("results").addEventListener("keydown", (e) => {
  const i = [...$("results").querySelectorAll(".file")].indexOf(document.activeElement);
  if (e.key === "ArrowDown" || e.key === "ArrowUp") {
    e.preventDefault();
    selectResult(e.key === "ArrowDown" ? Math.min(i + 1, $("results").querySelectorAll(".file").length - 1) : i - 1);
  } else if (e.key === "Escape") {
    $("search").value = "";
    renderResults();
    $("search").focus();
  }
});

/** Edit on a shared session opens your own file in it, made when you have none there yet. Returns false when it can't. */
async function ownFile() {
  const path = editPath();
  if (note(path) && !note(path).parts) return true;
  if (!myFile(current)) {
    say("Pick your character in Settings > General first");
    return false;
  }
  await invoke("create_file", { path, content: playerFile(current, baseName(me()), sessionDate(note(current)) || today()) })
    .catch((err) => String(err).endsWith("already exists.") || say(`Couldn't edit: ${err}`));
  await refresh();
  return !!note(path);
}

$("toggle").addEventListener("click", async () => {
  if (current && isDmCopy(current)) return; // read-only
  if (editing) {
    await flush();
    if (held) return; // see open
    editing = false;
  } else {
    editChoice = null;
    if (!(await ownFile())) return;
    if (editPath() === myFile(current)) say(`Editing your own notes in ${baseName(current)}`);
    editing = true;
    base = note(editPath())?.content ?? "";
    ed().setValue(base, { reset: true });
  }
  render();
  if (editing) ed().focus();
});

/** Session notes | My notes | Private notes, while editing a shared session: saves, then edits the picked file (made on first pick). */
$("edit-switch").addEventListener("click", async (e) => {
  const choice = e.target.closest("button")?.dataset.choice;
  if (!choice || editTarget(choicesFor(current), choice) === editPath()) return;
  await flush();
  if (inConflict) return say("Keep your version or load theirs first.");
  const was = editChoice;
  editChoice = choice;
  if (!(await ownFile())) editChoice = was;
  base = note(editPath())?.content ?? "";
  ed().setValue(base, { reset: true });
  render();
  ed().focus();
});

// ---------- deleting, and undoing it ----------

/** Asks before moving a page to the Trash, naming the `files` that go when they aren't just the page; true when confirmed. Focus starts on Cancel. */
function confirmDelete(path, files = [path]) {
  const n = backlinks(path, vault.notes).length;
  $("delete-title").textContent = `Move "${baseName(path)}" to the Trash?`;
  $("delete-files").textContent = `Your notes in it go: ${files.map((f) => (isPrivate(f) ? `your private notes, ${f}` : f)).join(" and ")}.`;
  $("delete-files").hidden = files[0] === path;
  $("delete-links").textContent = n ? `${n} page${n === 1 ? " links" : "s link"} to it; those links will show as missing.` : "";
  $("delete-links").hidden = !n;
  $("delete-dialog").returnValue = "";
  $("delete-dialog").showModal();
  $("delete-cancel").focus();
  return new Promise((done) =>
    $("delete-dialog").addEventListener("close", () => done($("delete-dialog").returnValue === "delete"), { once: true }));
}

/**
 * Moves a page (its `files`: a shared session's are the ones in it) to the OS Trash and returns what each held (for
 * undo), saving pending typing first so a failed delete loses nothing. When it was the open page, goes Back (skipping
 * it), or Home when there's nothing to go back to.
 */
async function trash(path, files = [path]) {
  const wasOpen = path === current;
  await flush();
  const contents = files.map((f) => note(f)?.content ?? "");
  const back = nav.find(-1, (p) => p !== path && alive(p), path);
  try {
    for (const file of files) await invoke("delete_file", { path: file });
  } finally {
    await refresh();
  }
  if (wasOpen) await (back >= 0 ? open(nav.entry(back), { to: back }) : open(null));
  return contents;
}

/**
 * Undo for creating a page (or redo for deleting one): only while it still holds `expected` (a shared session's
 * `files` each theirs, in order), so no work is lost.
 */
async function trashIfUnchanged(path, expected, files = [path]) {
  await flush();
  const want = [].concat(expected);
  if (files.every((f) => !note(f))) return; // already gone
  if (files.some((f, i) => note(f)?.content !== want[i])) throw `${baseName(path)} has changed since, so it was kept`;
  await trash(path, files);
}

/** Puts a page back at its path; never overwrites one that's there now. */
async function restore(path, content) {
  await invoke("create_file", { path, content }); // fails with "<path> already exists."
  await refresh();
}

async function deletePage(path) {
  if (modal()) return say("Close the open dialog first.");
  if (!note(path)) return say("Open a page to move it to the Trash.");
  if (isDmCopy(path)) return say("A player's private note is read-only here.");
  const files = note(path).parts ? ownSession(path) : [path];
  if (!files.length) return say("A shared session holds everyone's notes. Delete your own notes from its Timeline instead.");
  if (!(await confirmDelete(path, files))) return;
  const name = baseName(path);
  try {
    const contents = await trash(path, files);
    record({
      label: `Delete ${name}`,
      undo: async () => { for (const [i, f] of files.entries()) await restore(f, contents[i]); },
      redo: () => trashIfUnchanged(path, contents, files),
    });
    say(`Moved ${name} to the Trash`);
  } catch (err) {
    say(`Couldn't delete ${name}: ${err}`);
  }
}
$("delete").addEventListener("click", () => deletePage(current));
$("back").addEventListener("click", () => go(-1));
$("forward").addEventListener("click", () => go(1));

/** Rewrites a page from the reading view through the conflict-safe save (hotkey notes added meanwhile are kept). */
async function rewrite(path, change) {
  await flush();
  const n = note(path);
  if (!n) throw `${baseName(path)} no longer exists`;
  let written;
  try {
    written = await invoke("save_file", { path, content: change(n.content), base: n.content });
  } catch (err) {
    if (err !== "conflict") throw err;
    await refresh(); // the conflict banner is for the editor; here, show the page as it is now and change nothing
    throw "the page changed outside the app, so nothing was changed";
  }
  n.content = written; // a shared session's part too: its page reads its files' text
  if (path === editPath()) {
    base = written;
    if (editing) ed().setValue(written);
  }
  render();
}

/** Removes one note's line from a session, with an Undo toast. */
async function deleteNote(path, line) {
  const raw = note(path)?.content.split("\n")[line];
  if (raw === undefined) return;
  try {
    await rewrite(path, (md) => removeLine(md, line, raw));
  } catch (err) {
    return say(`Couldn't delete the note: ${err}`);
  }
  // ponytail: undo puts the line back by index; edits above it in the meantime would shift where it lands.
  record({ label: "Delete note", undo: () => rewrite(path, (md) => insertLine(md, line, raw)), redo: () => rewrite(path, (md) => removeLine(md, line, raw)) });
  showToast("Note deleted.");
}

let toastTimer;
function showToast(text) {
  $("toast-text").textContent = text;
  $("toast").hidden = false;
  clearTimeout(toastTimer);
  const later = () => (toastTimer = setTimeout(() => ($("toast").contains(document.activeElement) ? later() : hideToast()), 6000));
  later();
}
function hideToast() {
  clearTimeout(toastTimer);
  $("toast").hidden = true;
}
$("toast-undo").addEventListener("click", () => appHistory("undo")); // the newest action is the one the toast is about

/** Adds an undoable action; an older toast no longer applies. */
function record(action) {
  undos.push(action);
  hideToast();
}

let undoing = false;
async function appHistory(dir) {
  if (undoing) return;
  undoing = true;
  hideToast();
  try {
    const action = await undos[dir]();
    say(action ? `${dir === "undo" ? "Undid" : "Redid"}: ${action.label}` : `Nothing to ${dir}`);
  } catch (err) {
    say(`Couldn't ${dir}: ${err}`);
  } finally {
    undoing = false;
  }
}

/** Undo / Redo go where the focus is: the editor's own history, a text field's, else the app's actions. */
function undoRedo(dir) {
  const el = document.activeElement;
  if (editor && $("editor").contains(el)) ed()[dir]();
  else if (el?.matches("input:not([type=radio]), textarea")) document.execCommand(dir);
  else if (!modal()) appHistory(dir);
}

// ---------- renaming and moving ----------

let renaming = null; // the page the Rename dialog is for
const dirOf = (path) => path.split("/").slice(0, -1).join("/");

/** The Rename dialog, which also moves: `move` opens it as Move, with the folder list focused. */
function openRenameDialog(path, move = false) {
  if (modal()) return say("Close the open dialog first.");
  if (!note(path)) return say(`Open a page to ${move ? "move" : "rename"} it.`);
  if (keepsPlace(path)) return say(SESSIONS_STAY);
  if (isDmCopy(path)) return say("A player's private note is read-only here.");
  renaming = path;
  $("rename-title").textContent = move ? "Move page" : "Rename page";
  $("rename-confirm").textContent = move ? "Move" : "Rename";
  $("rename-name").value = baseName(path);
  // Only sessions go in Sessions/ (a page already there can stay); Templates/ and Attachments/ aren't for pages, as in the tree.
  // Folders as the sidebar shows them: a private page stays private wherever it goes (see moveTarget).
  const here = shownAt(dirOf(path));
  const folders = [...new Set(["", ...vault.folders.map(shownAt)])].sort(naturally).filter((f) => f === here || !/^(sessions|templates|attachments)(\/|$)/i.test(f));
  $("rename-folder").replaceChildren(...folders.map((f) => new Option(f || "Top level", f, false, f === here)));
  $("rename-error").textContent = "";
  $("rename-dialog").showModal();
  move ? $("rename-folder").focus() : $("rename-name").select();
}

/**
 * Renames page `from` to `to` and points every link to it at the new name, saving pending typing first. `exact`
 * ([path, now, then], for undo) puts a note back as it was while it still holds `now`; one changed since gets its links
 * rewritten instead. Returns the rewrites as [path after the rename, before, after], and the pages that couldn't be saved.
 */
async function renamePage(from, to, exact = []) {
  await flush();
  await loadVault(); // rewrite what's on disk now, so the saves below don't conflict
  const known = paths();
  const edits = [];
  // Links in a shared session's players' files too. Renaming a private page touches only private pages (its new name
  // mustn't reach shared ones); a DM's copies are never written.
  const editable = (path) => !isDmCopy(path) && (!isPrivate(from) || isPrivate(path));
  for (const { path, content } of [...vault.files, ...templates].filter((f) => editable(f.path))) {
    const back = exact.find(([p, now]) => p === path && now === content);
    const next = back ? back[2] : renameLinks(content, from, to, known);
    if (next !== content) edits.push([path === from ? to : path, content, next]);
  }
  await invoke("rename_file", { from, to }); // refuses an existing page before any note changes
  nav.rename(from, to);
  if (current === from) current = to;
  const failed = [];
  for (const edit of edits) {
    const [path, base, content] = edit;
    await invoke("save_file", { path, content, base }).catch(() => failed.push(edit));
  }
  await refresh();
  return { edits: edits.filter((e) => !failed.includes(e)), failed: failed.map(([p]) => baseName(p)) };
}

/** Renames or moves page `from` to `to` (see renamePage), undoably, and says so; throws when refused, links untouched. */
async function renameOrMove(from, to) {
  const { edits, failed } = await renamePage(from, to);
  const moved = dirOf(from) !== dirOf(to);
  const verb = moved ? "Move" : "Rename";
  // A folder closed before the move opens, so the moved page shows selected in the tree.
  for (let f = dirOf(to); f; f = dirOf(f)) for (const name of [f, shownAt(f)]) closedFolders.delete(name); // by either name (treeHtml)
  renderTree();
  // ponytail: undo / redo report only their label, so a page whose links couldn't be saved then goes unmentioned.
  record({ label: `${verb} ${baseName(from)}`, undo: () => renamePage(to, from, edits.map(([p, was, now]) => [p, now, was])), redo: () => renamePage(from, to) });
  const n = edits.filter(([p]) => p !== to).length;
  const done = moved ? `Moved ${baseName(to)} to ${shownAt(dirOf(to)) || "the top level"}` : `Renamed to ${baseName(to)}`; // as the sidebar names it
  say(failed.length ? `${verb}d, but couldn't update the links in ${failed.join(", ")}`
    : `${done}${n ? `; links updated in ${n} page${n === 1 ? "" : "s"}` : ""}`);
}

$("rename-cancel").addEventListener("click", () => $("rename-dialog").close());
$("rename-form").addEventListener("submit", async (e) => {
  if (e.submitter?.value !== "rename") return;
  e.preventDefault();
  const from = renaming;
  const name = $("rename-name").value.trim();
  // Never across the private line (Make private / Make shared ask first): the same rules as a drop, but the name's own.
  const folder = moveTarget(from, $("rename-folder").value);
  const problem = badName(name) || moveProblem(from, folder, []);
  if (problem) {
    $("rename-error").textContent = problem;
    return $("rename-name").focus();
  }
  const to = movedPath(`${name}.md`, folder);
  if (to === from) return $("rename-dialog").close();
  try {
    await renameOrMove(from, to);
  } catch (err) {
    return ($("rename-error").textContent = err);
  }
  $("rename-dialog").close();
});

// Drag a page in the sidebar onto a folder, or onto the tree outside any folder for the top level; Move… (the Rename
// dialog) does the same from the keyboard. dragDropEnabled is off in tauri.conf.json, so the webview gets these events.
let dragged = null; // the page being dragged
let dropMark = null; // the folder (or the tree) highlighted as where it would go
/** Where page `page` dropped on `el` goes: that folder, inside Private/ for a private page (moveTarget); a DM's copies stay as they are, refused. */
const dropFolder = (el, page) => {
  const folder = el.closest?.("details[data-folder]")?.dataset.folder ?? "";
  return folder === "dm" || isDmCopy(`${folder}/`) ? folder : moveTarget(page, folder);
};
function markDrop(el, refused) {
  dropMark?.classList.remove("drop-target", "drop-refused");
  dropMark = el;
  el?.classList.add(refused ? "drop-refused" : "drop-target");
}
let moveErrorTimer;
function moveError(text) {
  $("move-error").textContent = text;
  clearTimeout(moveErrorTimer);
  moveErrorTimer = setTimeout(() => ($("move-error").textContent = ""), 8000);
}
$("tree").addEventListener("dragstart", (e) => {
  dragged = e.target.closest?.(".file")?.dataset.path ?? null;
  if (!dragged) return;
  e.dataTransfer.setData("application/x-lorekeeper-page", dragged); // not text: dropped in the editor, it inserts nothing
  e.dataTransfer.effectAllowed = "move";
});
// A file from outside isn't a page: a drag the tree lost track of (re-rendered mid-drag) never moves one for it.
const pageDrag = (e) => dragged && !e.dataTransfer.types.includes("Files");
$("tree").addEventListener("dragover", (e) => {
  if (!pageDrag(e)) return;
  e.preventDefault(); // a refused folder still takes the drop, to say why
  const folder = dropFolder(e.target, dragged);
  const same = folder === dirOf(dragged);
  markDrop(same ? null : e.target.closest?.("details[data-folder]") ?? $("tree"), !!moveProblem(dragged, folder, paths()));
  e.dataTransfer.dropEffect = same ? "none" : "move";
});
$("tree").addEventListener("dragleave", (e) => $("tree").contains(e.relatedTarget) || markDrop(null));
document.addEventListener("dragend", () => {
  dragged = null;
  markDrop(null);
});
$("tree").addEventListener("drop", async (e) => {
  const from = dragged;
  if (!pageDrag(e)) return;
  e.preventDefault();
  dragged = null;
  markDrop(null);
  const folder = dropFolder(e.target, from);
  if (folder === dirOf(from)) return;
  const problem = moveProblem(from, folder, paths());
  if (problem) return moveError(`Can't move ${baseName(from)}: ${problem}`);
  moveError("");
  await renameOrMove(from, movedPath(from, folder)).catch((err) => moveError(`Can't move ${baseName(from)}: ${err}`));
});

// ---------- private notes: making a page private or shared, and the notice when who reads them changes ----------

/**
 * The header's Make private / Make shared button, in a campaign shared with your party, for a page that can move: not a
 * session (they keep their place) and not a DM's copy of a player's note.
 */
function showPrivacy(n) {
  const label = privateLabel(sharing());
  const movable = !!n && !!label && !n.parts && !isDmCopy(current) && !isSession(unprivate(current));
  $("privacy").hidden = !movable;
  if (!movable) return;
  delete $("privacy").dataset.sure;
  $("privacy").innerHTML = icon(isPrivate(current) ? "unlock" : "lock");
  $("privacy").setAttribute("aria-label", isPrivate(current) ? "Make shared" : "Make private");
  $("privacy").title = isPrivate(current) ? "Make shared: move this page out of Private/, and the whole party gets it"
    : `Make private: move this page into Private/ (${label}). Anyone who already synced it keeps the version they have`;
}

/**
 * Moves a page into Private/ or out of it. Sync sees a deletion on one side and a new page on the other. Links aren't
 * rewritten: they find the page by its name either way, and a shared page must not learn a private page's path.
 */
async function movePrivacy(from, to) {
  await flush();
  await invoke("rename_file", { from, to });
  nav.rename(from, to);
  if (current === from) current = to;
  await refresh();
}

$("privacy").addEventListener("click", async () => {
  const from = current;
  if (!from || !note(from)) return;
  const to = isPrivate(from) ? unprivate(from) : `Private/${from}`;
  // Sharing a private page is for everyone at once: the first click asks.
  if (isPrivate(from) && $("privacy").dataset.sure !== "yes") {
    $("privacy").dataset.sure = "yes";
    $("privacy").textContent = "Share it with the party?"; // the next render puts the icon back
    $("privacy").setAttribute("aria-label", "Share it with the party?");
    return;
  }
  try {
    await movePrivacy(from, to);
  } catch (err) {
    return say(String(err));
  }
  // Undo and redo never share a page (it may have private changes by then): only the button, which asks first, does.
  const share = () => { throw "that would share the page with the party; use Make shared, which asks first"; };
  const [back, again] = [() => movePrivacy(to, from), () => movePrivacy(from, to)];
  record({ label: isPrivate(to) ? "Make private" : "Make shared", undo: isPrivate(to) ? share : back, redo: isPrivate(to) ? again : share });
  // It was shared until now: players who synced it keep what they got (sync moves it to their trash), so say so.
  say(isPrivate(to) ? `${baseName(to)} is private from now on (${ownLabel()}); anyone who already synced it keeps the version they have`
    : `${baseName(to)} is shared with the party now`);
});

/** Once, after who reads private notes in the open campaign changed (sync.rs raises it, Settings or OK clears it). */
function showNotice() {
  const sh = sharing();
  $("notice").hidden = !sh?.privateNotice;
  if (sh?.privateNotice) $("notice-text").textContent = `Who reads private notes in this campaign changed. ${privateLabel(sh)}.`;
}

$("notice-ok").addEventListener("click", async () => {
  const path = settings.vaultPath, sh = sharing();
  $("notice").hidden = true;
  if (!sh) return;
  await invoke("save_settings", { settings: { ...settings, sharing: { ...settings.sharing, [path]: { ...sh, privateNotice: false } } } }).catch(say);
});

$("new-page").addEventListener("click", () => openNewDialog());
$("new-session").addEventListener("click", async () => {
  try {
    const path = await invoke("start_session"); // in a shared session, your own file in it
    await refresh();
    const content = note(path)?.content ?? "";
    const page = pageFor(path);
    record({ label: `Start ${baseName(page)}`, undo: () => trashIfUnchanged(path, content), redo: () => restore(path, content) });
    open(page);
  } catch (err) {
    say(`Couldn't start a session: ${err}`);
  }
});

$("take-theirs").addEventListener("click", async () => {
  inConflict = false;
  $("conflict").hidden = true;
  await refresh();
  base = note(editPath())?.content ?? "";
  ed().setValue(base);
  render();
});
$("keep-mine").addEventListener("click", async () => {
  inConflict = false;
  $("conflict").hidden = true;
  const mine = ed().getValue();
  await loadVault();
  base = note(editPath())?.content ?? ""; // save over what's on disk now
  ed().setValue(mine); // a refresh during the await may have loaded theirs
  saving = save().finally(() => (saving = null));
});

// ---------- menu bar, shortcuts, context menu ----------

/** Help > Cheat Sheet: note symbols, links and shortcuts, with the global shortcuts as set now. */
function openCheatSheet() {
  $("cheat-body").innerHTML = cheatSheetHtml({ settings, mac });
  $("cheat-body").scrollTop = 0;
  $("cheat-dialog").showModal();
}

const zoomKey = (key) => () => window.dispatchEvent(new KeyboardEvent("keydown", { key, metaKey: mac, ctrlKey: !mac }));

/** What the menu bar and the keyboard shortcuts do; each reuses the button or function behind it. */
const actions = {
  search: () => $("search").focus(),
  toggle: () => current && $("toggle").click(),
  newPage: () => $("new-dialog").open || (haveCampaign() && openNewDialog()),
  newSession: () => haveCampaign() && $("new-session").click(),
  joinCampaign: () => invoke("open_settings").then(() => emitTo("settings", "open-join")).catch(say),
  settings: () => invoke("open_settings").catch(say),
  cheatSheet: () => ($("cheat-dialog").open ? $("cheat-dialog").close() : modal() || openCheatSheet()),
  home: () => modal() || open(null),
  back: () => modal() || go(-1),
  forward: () => modal() || go(1),
  deletePage: () => deletePage(current),
  renamePage: () => openRenameDialog(current),
  movePage: () => openRenameDialog(current, true),
  undo: () => undoRedo("undo"),
  redo: () => undoRedo("redo"),
  // Tauri's zoom-hotkey.js (zoomHotkeysEnabled, macOS/Linux) owns the zoom level; drive it with the key it listens for.
  zoomIn: zoomKey("="),
  zoomOut: zoomKey("-"),
  zoomReset: zoomKey("0"),
};
const ZOOM = { "=": "zoomIn", "+": "zoomIn", "-": "zoomOut", "0": "zoomReset" };

let last = { name: "", at: 0, from: "" };
/**
 * Runs an action once per key press: where both the keydown and the menu accelerator arrive, the second is dropped.
 * Only a repeat from the other source counts, so quick presses of one key (undo, undo, undo) all run.
 */
function run(name, from = "key") {
  const now = performance.now();
  if (name === last.name && from !== last.from && now - last.at < 300) return;
  last = { name, at: now, from };
  if (name !== "settings") {
    // The macOS menu is app-wide: Cmd+N typed in the quick-note box must not act on this window while it's hidden.
    if (document.visibilityState === "hidden") return;
    if (!document.hasFocus()) tauriWindow?.setFocus().catch(() => {});
  }
  actions[name]();
}

document.addEventListener("keydown", (e) => {
  if (e.key === "F2") {
    e.preventDefault();
    return run("renamePage");
  }
  if (!(e.metaKey || e.ctrlKey)) return;
  const key = e.key.toLowerCase();
  // The zoom polyfill zooms on this same keydown; marking it handled stops the View menu accelerator zooming again.
  if (!windows && ZOOM[key]) {
    last = { name: ZOOM[key], at: performance.now(), from: "key" };
    return e.preventDefault();
  }
  const typing = e.target.closest?.("#editor, input, textarea");
  let name;
  if (key === "z" || (key === "y" && !mac)) name = key === "z" && !e.shiftKey ? "undo" : "redo"; // routed by focus in undoRedo
  else if (key === "backspace") name = typing ? "" : "deletePage"; // in text, it deletes to the line start
  else name = (e.shiftKey && { n: "newSession", h: "home" }[key]) || { k: "search", e: "toggle", n: "newPage", ",": "settings", "[": "back", "]": "forward", "/": "cheatSheet" }[key];
  if (!name) return;
  e.preventDefault(); // also keeps the matching menu accelerator from firing on macOS
  run(name);
});

/**
 * A native menu from plain item options. An item with an action becomes its own MenuItem (CheckMenuItem) first:
 * Menu.new drops the items it builds from plain options as soon as it returns, and their action handlers with them
 * (tauri 2.12 menu/mod.rs), so a click on one did nothing.
 */
async function nativeMenu({ items }) {
  const { Menu, MenuItem, CheckMenuItem } = window.__TAURI__.menu;
  const keep = async (i) => i.action ? ("checked" in i ? CheckMenuItem : MenuItem).new(i)
    : i.items ? { ...i, items: await Promise.all(i.items.map(keep)) } : i;
  return Menu.new({ items: await Promise.all(items.map(keep)) });
}

/** The native menu bar: app-wide on macOS (replacing Tauri's default one), the main window's own menu bar elsewhere. */
async function buildMenu() {
  if (!window.__TAURI__.menu) return;
  const sep = { item: "Separator" };
  const linuxOk = ["Separator", "Cut", "Copy", "Paste", "SelectAll"]; // GTK greys out the other predefined items
  const predefined = (...names) => names.filter((n) => !linux || linuxOk.includes(n)).map((item) => ({ item }));
  const item = (text, name, accelerator) => ({ text, accelerator, action: () => run(name, "menu") });
  const version = await window.__TAURI__.app?.getVersion().catch(() => undefined);
  const menu = await nativeMenu({
    items: [
      { text: "Lorekeeper", items: [
        { item: { About: { name: "Lorekeeper", version } } },
        sep,
        item("Settings…", "settings", "CmdOrCtrl+,"),
        ...(mac ? [sep, { item: "Services" }, sep, ...predefined("Hide", "HideOthers", "ShowAll"), sep, { item: "Quit" }] : []),
      ] },
      { text: "File", items: [
        item("New Page", "newPage", "CmdOrCtrl+N"),
        item("New Session", "newSession", "CmdOrCtrl+Shift+N"),
        sep,
        item("Rename…", "renamePage", "F2"),
        item("Move to…", "movePage"),
        item("Move to Trash…", "deletePage"), // ⌘⌫ comes from the keydown handler, so text fields keep it
      ] },
      // Undo and Redo are our own items, not the predefined ones: the editor's history and the app's deletions need them.
      { text: "Edit", items: [item("Undo", "undo", "CmdOrCtrl+Z"), item("Redo", "redo", "CmdOrCtrl+Shift+Z"), ...predefined("Separator", "Cut", "Copy", "Paste", "SelectAll")] },
      { text: "View", items: [
        item("Home", "home", "CmdOrCtrl+Shift+H"),
        item("Back", "back", "CmdOrCtrl+["),
        item("Forward", "forward", "CmdOrCtrl+]"),
        sep,
        item("Edit / Preview", "toggle", "CmdOrCtrl+E"),
        item("Search", "search", "CmdOrCtrl+K"),
        // Windows zooms natively in WebView2, with no level the page can drive.
        ...(windows ? [] : [sep, item("Zoom In", "zoomIn", "CmdOrCtrl+="), item("Zoom Out", "zoomOut", "CmdOrCtrl+-"), item("Actual Size", "zoomReset", "CmdOrCtrl+0")]),
        ...(mac ? [sep, { item: "Fullscreen" }] : []),
      ] },
      // Tauri's Window-menu id: set_menu registers it with AppKit, which adds the open-window list.
      ...(linux ? [] : [{ id: "__tauri_window_menu__", text: "Window", items: predefined("Minimize", "Maximize", "Separator", "CloseWindow") }]),
      { text: "Help", items: [item("Cheat Sheet", "cheatSheet", "CmdOrCtrl+/")] },
    ],
  });
  await (mac ? menu.setAsAppMenu() : menu.setAsWindowMenu());
}

function copyLink(path) {
  const text = `[[${baseName(path)}]]`;
  // A native menu click isn't a user gesture, so WebKit may refuse; then the clipboard plugin writes it, as plain text.
  navigator.clipboard.writeText(text)
    .catch(() => invoke("plugin:clipboard-manager|write_text", { text }))
    .then(() => say(`Copied ${text}`), (err) => say(`Copy failed: ${err}`));
}

$("sidebar").addEventListener("contextmenu", (e) => {
  const file = e.target.closest(".file")?.dataset.path;
  const folder = e.target.closest("summary")?.parentElement.dataset.folder;
  if (!window.__TAURI__.menu || (file === undefined && folder === undefined)) return;
  e.preventDefault();
  const reveal = mac ? "Show Vault in Finder" : windows ? "Show Vault in Explorer" : "Open Vault Folder";
  const items = file !== undefined
    ? [
        { text: "Open", action: () => open(file) },
        { text: reveal, action: () => invoke("open_vault_folder").catch(say) },
        { item: "Separator" },
        { text: "Copy Link", action: () => copyLink(file) },
        // Only what works for this page: sessions keep their name and folder, a shared one holds everyone's notes
        // (unless it's all yours).
        ...(note(file)?.parts && !ownSession(file).length ? [] : [
          { item: "Separator" },
          ...(keepsPlace(file) ? [] : [
            { text: "Rename…", action: () => openRenameDialog(file) },
            { text: "Move to…", action: () => openRenameDialog(file, true) },
          ]),
          { text: "Delete…", action: () => deletePage(file) },
        ]),
      ]
    : /^sessions(\/|$)/i.test(folder) // only sessions go in Sessions/ (moveProblem)
      ? [{ text: "New Session", action: () => actions.newSession() }]
      : [{ text: "New Page Here…", action: () => openNewDialog("", { folder }) }];
  // ponytail: each right-click's menu and items stay open (closing them once popup() returns could drop a click still on
  // its way, see nativeMenu): a few small resources each time. Close the previous menu's on the next right-click if that matters.
  nativeMenu({ items }).then((m) => m.popup()).catch(say);
});

// The webview's own menu (Reload, Back, Print, Inspect…) is for web pages: only text keeps it, for Cut, Copy, Paste and
// spelling, and so does selected text anywhere, for Copy. The sidebar's pages and folders get the app's menu above.
document.addEventListener("contextmenu", (e) =>
  e.target.closest?.("#editor, textarea, input:not([type=checkbox], [type=radio])") || getSelection()?.toString() || e.preventDefault());

if (mac) {
  // The title bar overlays the page (tauri.conf.json): the sidebar's top strip and the header move the window.
  $("sidebar").dataset.tauriDragRegion = ""; // only its own padding, not the rows inside it
  $("bar").dataset.tauriDragRegion = "deep"; // anywhere but its buttons
}

/** Theme, editor font size and the default session view from the Settings window. */
function applySettings(next) {
  const prev = settings;
  settings = { ...settings, ...next };
  applyTheme(settings.theme);
  // Native appearance follows too (the sidebar material, title bar, menus); on macOS this is app-wide.
  tauriWindow?.setTheme(nativeTheme(settings.theme)).catch(() => {});
  editor?.setFontSize(settings.editorFontSize);
  if (prev.vaultPath !== undefined && settings.vaultPath !== prev.vaultPath) {
    forgetPictures();
    refresh();
  }
  else render();
  $("campaign").textContent = settings.vaultPath ? campaignName(settings.vaultPath) : "No campaign";
  showSync();
  showNotice();
  showMoveOffer();
}

// ---------- the Lorekeeper folder: Templates/ and a folder per campaign ----------

$("view").addEventListener("submit", async (e) => {
  if (e.target.id !== "first-run-form") return;
  e.preventDefault();
  $("first-run-error").textContent = "";
  try {
    await switchCampaign(await invoke("create_campaign", { name: $("first-run-name").value }));
  } catch (err) {
    $("first-run-error").textContent = String(err);
  }
});

/** Offers, until you answer, to move a campaign kept in the Lorekeeper folder itself into a folder of its own. */
async function showMoveOffer() {
  const offer = settings.vaultPath ? await invoke("move_offer").catch(() => null) : null;
  const shown = !$("move-offer").hidden;
  $("move-offer").hidden = startView(settings, offer) !== "move";
  if (offer && !shown) $("move-name").value = offer;
}

$("move-offer").addEventListener("submit", async (e) => {
  e.preventDefault();
  $("move-offer-error").textContent = "";
  await flush(); // the open page is saved where it is, then moves with the rest
  if (inConflict) return ($("move-offer-error").textContent = "Keep your version or load theirs first.");
  try {
    const path = await invoke("move_campaign", { name: $("move-name").value });
    $("move-offer").hidden = true;
    say(`Moved your notes to ${path}`);
  } catch (err) {
    $("move-offer-error").textContent = String(err);
  }
});
$("move-no").addEventListener("click", () => {
  $("move-offer").hidden = true;
  invoke("save_settings", { settings: { ...settings, moveDeclined: true } }).catch(say);
});

// ---------- a shared campaign's sync status and who's online (shared.rs), in the sidebar's footer ----------

const syncSnapshots = new Map();
/** Teammates' names come from the network: set as text only. */
function showSync() {
  const path = settings.vaultPath, snap = syncSnapshots.get(path);
  const text = [syncText(snap, settings.sharing?.[path], campaignName(path)), onlineText(snap)].filter(Boolean).join(" · ");
  $("sync-line").textContent = text;
  $("sync-line").hidden = !text;
}
listen("sync-status", ({ payload }) => {
  syncSnapshots.set(payload.path, payload);
  if (payload.path === settings.vaultPath) showSync();
});
invoke("sync_info").then((info) => {
  for (const snap of info.statuses) syncSnapshots.set(snap.path, snap);
  showSync();
}).catch(() => {});

// ---------- campaigns: one notes folder each ----------

/** A campaign's name: the one given in Settings, else its notes folder's name. */
function campaignName(path) {
  return settings.backupNames?.[path] || path?.split(/[\\/]/).filter(Boolean).pop() || "Your campaign";
}

/** Saves the open page into the campaign it belongs to, then switches; Home shows the new campaign. */
async function switchCampaign(path) {
  if (path === settings.vaultPath) return;
  await flush();
  if (inConflict) return say("Keep your version or load theirs before switching campaigns.");
  try {
    await invoke("switch_campaign", { path });
  } catch (err) {
    return say(String(err));
  }
  open(null);
}

$("campaign").addEventListener("click", () => {
  if (!window.__TAURI__.menu) return;
  const items = (settings.campaigns ?? []).map((path) =>
    ({ text: campaignName(path), checked: path === settings.vaultPath, action: () => switchCampaign(path) }));
  const manage = { text: "Add or Remove Campaigns…", action: () => run("settings", "menu") };
  nativeMenu({ items: [...items, { item: "Separator" }, manage] }).then((m) => m.popup()).catch(say);
});
// From the tray menu and from Settings after adding a campaign.
listen("switch-campaign", (e) => switchCampaign(e.payload));

// Shortcuts in tooltips and the search box, the way this OS writes them (⌘K on macOS, Ctrl+K elsewhere).
for (const el of document.querySelectorAll("[data-shortcut]")) {
  const keys = readable(el.dataset.shortcut, mac);
  if (el.placeholder) el.placeholder += `  ${keys}`;
  else el.title += ` (${keys})`;
}
$("home").insertAdjacentHTML("afterbegin", icon("home"));
$("new-page").insertAdjacentHTML("afterbegin", icon("note"));
$("new-session").insertAdjacentHTML("afterbegin", icon("session"));
$("back").innerHTML = icon("back");
$("forward").innerHTML = icon("forward");
$("delete").innerHTML = icon("trash");
$("notice-ok").innerHTML = icon("close");
$("connections").querySelector(".graph-fit").innerHTML = icon("fit");
// A pinch is Ctrl+scroll, which Tauri's zoom script (and WebView2) turn into zooming the whole window, a step per
// event; the map takes pinches for itself first. The app zooms with Cmd/Ctrl +, - and 0 only.
document.addEventListener("wheel", (e) => {
  if (!e.ctrlKey) return;
  e.preventDefault();
  e.stopPropagation(); // Tauri's listener is on window, past the document
}, { passive: false });
// Mouse back / forward buttons.
window.addEventListener("mouseup", (e) => {
  if (e.button !== 3 && e.button !== 4) return;
  e.preventDefault();
  run(e.button === 3 ? "back" : "forward", "mouse");
});

$("editor-hint").innerHTML = NOTE_PREFIXES
  .map(({ keys: [k], kind }) => `<span>${escape(k)} ${icon(kind)}${KINDS[kind]}</span>`).join(" · ");

// Hotkey notes and edits made in Obsidian show up here without a manual reload.
listen("vault-changed", refresh);
listen("settings-changed", (e) => applySettings(e.payload));
// The global New Page hotkey (Rust shows this window first).
listen("new-page", actions.newPage);
listen("cheat-sheet", () => $("cheat-dialog").open || modal() || openCheatSheet()); // Settings > Shortcuts
window.addEventListener("focus", refresh);
window.addEventListener("beforeunload", flush);

applySettings(await invoke("get_settings").catch(() => ({})));
await refresh();
open(null); // Home

buildMenu().catch((err) => console.error("menu:", err));
