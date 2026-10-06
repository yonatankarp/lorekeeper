// The Lorekeeper window: browse the vault, read and edit pages, follow [[links]], search, backlinks.
import { marked } from "./vendor/marked.esm.js";
import { createEditor } from "./editor.js";
import { escape, insertLine, linkify, parse, removeLine, sessions, stripLinks, timeline, toHtml, toText } from "./notes.js";
import {
  backlinks, badName, baseName, buildTree, characterProps, dndBeyondId, fillTemplate, folderFor, openQuests, party, pcPageFor, questStatus,
  recentlyMentioned, renameLinks, kindOf, resolve, safePageName, search, sheetId, shownProps, splitFrontmatter,
} from "./vault.js";
import { navHistory, undoStack } from "./history.js";
import { applyTheme, nativeTheme } from "./theme.js";
import { icon } from "./icons.js";
import { ATTACHMENTS, freeName, imageLabel, isImage, pastedName, resolveImage, safeName } from "./images.js";

const { invoke, convertFileSrc } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);
const { platform } = document.documentElement.dataset; // set by platform.js
const mac = platform === "macos";
const windows = platform === "windows";
const linux = !mac && !windows;
const tauriWindow = window.__TAURI__.window?.getCurrentWindow();

let vault = { folders: [], notes: [], currentSession: "" };
const canObsidian = () => vault.hasObsidian || vault.obsidianInstalled; // installed but no vault yet: the button explains how
let templates = []; // Templates/ notes: used by "New page", hidden everywhere else
let current = null; // open page path; null is Home
const nav = navHistory(); // pages visited, for Back / Forward
const undos = undoStack(); // the app's own undoable actions (deleting, creating); text edits use the editor's history
let base = ""; // file content the editor started from, for conflict-safe saves
let editing = false;
let saveTimer = null;
let saving = null; // in-flight save promise
let inConflict = false;
const closedFolders = new Set();
const KINDS = { npc: ["@", "NPC"], loot: ["#", "Loot"], quest: ["!", "Quest"], mystery: ["?", "Mystery"], quote: ['"', "Quote"] };
let settings = { theme: "system", editorFontSize: 15, sessionView: "timeline" }; // until get_settings answers
let sessionView = settings.sessionView; // or "journal": the grouped D&D Beyond preview; the switch changes it until restart
let editor = null; // created on first Edit, then reused for every page

const note = (path) => vault.notes.find((n) => n.path === path);
const paths = () => vault.notes.map((n) => n.path);
const isSession = (path) => path?.startsWith("Sessions/");
const alive = (path) => path === null || !!note(path); // Home, or a page still in the vault
const modal = () => !!document.querySelector("dialog[open]");
const dirty = () => saveTimer !== null || saving !== null;
const today = () => new Date().toLocaleDateString("sv-SE"); // YYYY-MM-DD

/** The Markdown editor, created the first time it's needed. */
function ed() {
  if (!editor) {
    editor = createEditor($("editor"), {
      onChange: () => {
        scheduleSave();
        syncCopy();
      },
      onFollowLink: followLink,
      onImage: saveImage,
      pageNames: () => vault.notes.map((n) => baseName(n.path)),
    });
    editor.setFontSize(settings.editorFontSize);
  }
  return editor;
}

/** Reads the vault, keeping Templates/ out of the tree, search, links and backlinks. */
async function loadVault() {
  const v = await invoke("read_vault");
  const internal = (p) => p === "Templates" || p.startsWith("Templates/");
  templates = v.notes.filter((n) => internal(n.path));
  vault = { ...v, notes: v.notes.filter((n) => !internal(n.path)), folders: v.folders.filter((f) => !internal(f)) };
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

/** Raw text with its [[links]] made clickable and its ![[images]] shown. */
const inlineLinks = (text) => linkify(text, (target, label, m) => (m[1] && isImage(target) ? imageHtml(target, m[4] ?? target) : linkHtml(target, label)));

/** Status words that end a quest or a life get their own badge and icon. */
const STATUS_ICONS = { done: "done", failed: "failed", dead: "failed" };

/** "2026-10-04" as "4 Oct 2026"; anything else as written. */
const prettyDate = (v) =>
  /^\d{4}-\d{2}-\d{2}$/.test(v) ? new Date(`${v}T00:00`).toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" }) : null;

/** A page's properties, as a small stat block: see shownProps. */
function propsHtml(props, path) {
  const shown = shownProps(props, paths(), kindOf(path));
  const rows = shown.rows.map(({ key, label, value, icon: name, path }) => {
    let html;
    if (key === "status" || key === "rarity") {
      const word = value.toLowerCase();
      html = `<span class="${key} ${key}-${escape(word.replace(/[^a-z]/g, ""))}">${STATUS_ICONS[word] ? icon(STATUS_ICONS[word]) : ""}${escape(value)}</span>`;
    } else if (key === "dndbeyond") {
      // The sheet opens in the browser (the document click handler sends https links to open_url).
      html = sheetId(value)
        ? `<a href="${escape(value)}">Character sheet</a> <button type="button" class="ghost props-action" data-ddb-refresh ` +
          `title="Refresh from D&amp;D Beyond" aria-label="Refresh from D&amp;D Beyond">Refresh</button>`
        : inlineLinks(value);
    } else if (path) {
      html = `<a class="wikilink" data-target="${escape(value)}" href="#">${icon(name)}${escape(value)}</a>`;
    } else {
      const date = prettyDate(value);
      html = date ? `<time datetime="${escape(value)}">${escape(date)}</time>` : `${name ? icon(name) : ""}${inlineLinks(value)}`;
    }
    return `<dt>${escape(label)}</dt><dd>${html}</dd>`;
  });
  const sum = shown.summary ? `<p class="props-summary">${icon(kindOf(path))}${escape(shown.summary)}</p>` : "";
  return sum || rows.length ? `<div class="props">${sum}${rows.length ? `<dl>${rows.join("")}</dl>` : ""}</div>` : "";
}

// ---------- sidebar ----------

function treeHtml(node) {
  const dirs = node.dirs
    .map((d) => {
      const n = d.files.length;
      const count = n ? `<span class="sr-only">, </span>${n}<span class="sr-only"> page${n === 1 ? "" : "s"}</span>` : "";
      const empty = `<button type="button" class="folder-new" data-folder="${escape(d.path)}">+ New ${escape(typeFor(d.path) || "page")}</button>`;
      return `<details data-folder="${escape(d.path)}"${closedFolders.has(d.path) ? "" : " open"}>
        <summary>${escape(d.name)}<span class="count">${count}</span></summary>
        <div class="children">${treeHtml(d) || empty}</div></details>`;
    })
    .join("");
  const files = node.files
    .map((p) => {
      // Finished quests: dimmed, with a check or a cross, and the outcome in the accessible name.
      const status = p.startsWith("Quests/") ? questStatus(note(p)?.content ?? "") : "";
      const ended = status === "done" || status === "failed";
      return `<button class="file${p === current ? " active" : ""}${ended ? " quest-ended" : ""}"${p === current ? ' aria-current="page"' : ""} data-path="${escape(p)}" title="${escape(p)}">` +
        `${ended ? icon(status) : ""}${escape(baseName(p))}${ended ? `<span class="sr-only">, ${status}</span>` : ""}</button>`;
    })
    .join("");
  return dirs + files;
}

function renderTree() {
  const scroll = $("tree").scrollTop;
  $("tree").innerHTML = treeHtml(buildTree(vault.folders, paths()));
  $("tree").scrollTop = scroll;
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
          const folder = h.path.split("/").slice(0, -1).join("/");
          return `<button class="file" data-path="${escape(h.path)}">${mark(baseName(h.path))}
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
  `<dl class="prefix-legend">${Object.entries(KINDS).map(([kind, [k, label]]) => `<dt><kbd>${escape(k)}</kbd></dt><dd>${icon(kind)}${label}</dd>`).join("")}</dl>`;

const emptySessionHtml = `<div class="empty-state">${icon("session")}<h2>No notes yet</h2>
  <p>Press <kbd>⌘⌥N</kbd> for a quick note, or <kbd>⌘⇧S</kbd> to save selected text (or the clipboard).</p>
  <p>Notes appear here live during the game. Start a note with a symbol to file it:</p>${legendHtml}
  <p class="legend-note">A <kbd>!</kbd> note goes in this session's Quests section. To follow a quest across sessions, make a Quest page with + New page: open ones are listed at the top of every session.</p></div>`;

/** The notes in order; `removable` adds each row's delete button (the session's own Timeline view). */
const timelineHtml = (items, removable = false) =>
  `<ol class="timeline">${items
    .map(({ time, kind, text, line }) => {
      const badge = KINDS[kind] ? `<span class="kind kind-${kind}">${icon(kind)}${KINDS[kind][1]}</span>` : "";
      const remove = removable
        ? `<button type="button" class="remove-note" data-line="${line}" aria-label="Delete note: ${escape(stripLinks(text))}" title="Delete note">${icon("trash")}</button>`
        : "";
      return `<li><time>${escape(time)}</time>${badge}<span class="text">${inlineLinks(text)}</span>${remove}</li>`;
    })
    .join("")}</ol>`;

/** The Quests/ pages still open, at the top of every session; nothing when there are none. */
function openQuestsHtml() {
  const quests = openQuests(vault.notes);
  if (!quests.length) return "";
  const items = quests.map((p) => `<li><a data-path="${escape(p)}" href="#">${icon("quest")}${escape(baseName(p))}</a></li>`);
  return `<section class="open-quests" aria-labelledby="open-quests-title"><h2 id="open-quests-title">Open quests</h2><ul>${items.join("")}</ul></section>`;
}

function sessionHtml(content) {
  const session = parse(content);
  const items = timeline(content);
  const title = `<h1 class="session-title">${session.title ? inlineLinks(session.title) : escape(baseName(current))}</h1>`;
  if (!items.length) return title + openQuestsHtml() + emptySessionHtml;
  const pressed = (v) => `aria-pressed="${sessionView === v}"`;
  const views = `<div class="view-switch" role="group" aria-label="Session view">
    <button type="button" data-view="timeline" ${pressed("timeline")}>Timeline</button>
    <button type="button" data-view="journal" ${pressed("journal")}>Journal</button></div>`;
  return views + title + openQuestsHtml() + (sessionView === "journal"
    ? `<div class="journal">${toHtml({ ...session, title: "" }, linkHtml)}</div>` +
      `<p class="session-note">This is what “Copy for D&amp;D Beyond” pastes. Click Edit to see every line.</p>`
    : timelineHtml(items, true));
}

// ---------- home ----------

/** The campaign's contents page: latest session, open quests, the party, who and what came up lately, the sessions. */
function homeHtml() {
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
  if (latest) {
    const items = timeline(note(latest.path).content);
    cards.push(card("latest", "Latest session", "session", `
      <h3 class="home-latest">${escape(latest.title)}</h3>
      <p class="home-meta">${dated(latest, items.length)}</p>
      ${items.length ? timelineHtml(items.slice(-5)) : `<p class="home-none">No notes yet. Press <kbd>⌘⌥N</kbd> during the game to jot one.</p>`}
      <div class="home-actions">
        <a href="#" class="button-link" data-path="${escape(latest.path)}">Open session</a>
        <button type="button" class="seal" data-copy="${escape(latest.path)}"${items.length ? "" : " disabled"}>Copy for D&amp;D Beyond</button>
      </div>`, true));
  }
  if (quests.length) cards.push(card("quests", "Open quests", "quest", `<ul class="home-list">${quests.map((p) => `<li>${link(p, "quest")}</li>`).join("")}</ul>`));
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
    cards.push(card("sessions", "Sessions", "note", `<ul class="home-list contents">${items.join("")}</ul>
      <button type="button" class="ghost" data-action="newSession">+ New session</button>`));
  }
  if (!cards.length) {
    return `<div class="empty-state">${icon("home")}<h1>Welcome to Lorekeeper</h1>
      <p>This page gathers your campaign at a glance: the latest session, open quests, the party and who you've met.</p>
      <p>Start a session, then press <kbd>⌘⌥N</kbd> during the game to jot a note.</p>
      <p class="home-actions"><button type="button" class="seal" data-action="newSession">+ New session</button>
      <button type="button" class="ghost" data-action="newPage">+ New page</button></p></div>`;
  }
  const campaign = campaignName(settings.vaultPath);
  return `<h1 class="home-title">${escape(campaign)}</h1><div class="home-grid">${cards.join("")}</div>`;
}

/** Copy is pointless until the session has a note; while editing, the unsaved text counts. */
const syncCopy = () =>
  ($("copy").disabled = !timeline(editing ? ed().getValue() : note(current)?.content ?? "").length);

let shown = ""; // the page HTML on screen: re-rendering the same HTML would only lose keyboard focus
function setView(html) {
  if (html !== shown) $("view").innerHTML = shown = html;
}

function render() {
  const n = note(current); // none: Home
  const folder = current ? current.split("/").slice(0, -1).join(" / ") : "";
  $("crumbs").innerHTML = n
    ? `<a href="#" data-home>Home</a><span class="folder"> / ${folder ? `${escape(folder)} / ` : ""}</span><strong>${escape(baseName(current))}</strong>`
    : `<strong aria-current="page">Home</strong>`;
  $("home").classList.toggle("active", !n);
  n ? $("home").removeAttribute("aria-current") : $("home").setAttribute("aria-current", "page");
  $("back").disabled = nav.find(-1, alive, current) < 0;
  $("forward").disabled = nav.find(1, alive, current) < 0;
  $("delete").disabled = !n;
  $("rename").disabled = !n;
  $("backlinks").hidden = !n;
  $("toggle").hidden = !n;
  $("obsidian").hidden = !n || !canObsidian();
  $("copy").hidden = !n || !isSession(current);
  syncCopy();
  $("toggle").textContent = editing ? "Done" : "Edit";
  $("editor").hidden = !editing || !n;
  $("editor-hint").hidden = !editing || !n || !isSession(current);
  $("view").hidden = editing && !!n;

  if (!n) return setView(homeHtml());
  if (!editing) {
    const { props, body } = splitFrontmatter(n.content);
    setView(isSession(current) ? sessionHtml(n.content) : propsHtml(props, current) + marked.parse(body));
  }
  const links = backlinks(current, vault.notes);
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
  if (to === undefined) nav.visit(path);
  else nav.go(to);
  current = path;
  base = note(path)?.content ?? "";
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

async function flush() {
  if (saveTimer) {
    clearTimeout(saveTimer);
    saveTimer = null;
    saving = save().finally(() => (saving = null));
  }
  await saving;
}

async function save() {
  const path = current;
  const content = ed().getValue();
  try {
    const written = await invoke("save_file", { path, content, base });
    base = written;
    const n = note(path);
    if (n) n.content = written;
    if (path.startsWith("Quests/")) renderTree(); // a changed status moves the check mark
    // Notes added by the hotkeys while editing were kept on disk; show them in the editor too.
    if (written !== content && written.startsWith(content) && path === current) ed().append(written.slice(content.length));
    say("Saved");
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
  const n = note(current);
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
  newFrom = folder;
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
  const path = folder ? `${folder}/${name}.md` : `${name}.md`;
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
        await rewrite(page, (md) => characterProps(md, c));
        if (note(page).content !== before) changes.push([page, before, note(page).content]);
        continue;
      }
      const folder = folderOf("PC");
      const taken = (p) => vault.notes.some((n) => n.path.toLowerCase() === p.toLowerCase());
      let path = `${folder}/${name}.md`;
      if (taken(path)) path = `${folder}/${name} (${c.id}).md`; // a page of that name belongs to another character
      const tpl = templateOf("PC");
      const content = characterProps(tpl ? fillTemplate(tpl.content, name, today()) : `# ${name}\n\n`, c);
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
    await rewrite(path, (md) => characterProps(md, c));
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

$("view").addEventListener("click", async (e) => {
  const button = e.target.closest("button");
  if (button?.dataset.copy) return copySession(note(button.dataset.copy)?.content ?? "");
  if (button?.dataset.action) return actions[button.dataset.action]();
  if (button?.dataset.ddbRefresh !== undefined) return refreshCharacter(current, button);
  if (button?.classList.contains("remove-note")) {
    const row = [...$("view").querySelectorAll(".remove-note")].indexOf(button);
    await deleteNote(current, +button.dataset.line);
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

$("toggle").addEventListener("click", async () => {
  if (editing) {
    await flush();
    editing = false;
  } else {
    editing = true;
    base = note(current)?.content ?? "";
    ed().setValue(base, { reset: true });
  }
  render();
  if (editing) ed().focus();
});

/** Copies a session's notes, grouped, for the D&D Beyond journal (the header button, and the Latest session card on Home). */
function copySession(content) {
  const session = parse(content);
  return invoke("copy_html", { html: toHtml(session), text: toText(session) })
    .then(() => say("Copied. Paste it into the D&D Beyond journal"), (err) => say(`Copy failed: ${err}`));
}
$("copy").addEventListener("click", () => copySession(editing ? ed().getValue() : note(current)?.content ?? ""));

// ---------- deleting, and undoing it ----------

/** Asks before moving a page to the Trash; true when confirmed. Focus starts on Cancel. */
function confirmDelete(path) {
  const n = backlinks(path, vault.notes).length;
  $("delete-title").textContent = `Move "${baseName(path)}" to the Trash?`;
  $("delete-links").textContent = n ? `${n} page${n === 1 ? " links" : "s link"} to it; those links will show as missing.` : "";
  $("delete-links").hidden = !n;
  $("delete-dialog").returnValue = "";
  $("delete-dialog").showModal();
  $("delete-cancel").focus();
  return new Promise((done) =>
    $("delete-dialog").addEventListener("close", () => done($("delete-dialog").returnValue === "delete"), { once: true }));
}

/**
 * Moves a page to the OS Trash and returns what it held (for undo), saving pending typing first so a failed delete
 * loses nothing. When it was the open page, goes Back (skipping it), or Home when there's nothing to go back to.
 */
async function trash(path) {
  const wasOpen = path === current;
  await flush();
  const content = note(path)?.content ?? "";
  const back = nav.find(-1, (p) => p !== path && alive(p), path);
  await invoke("delete_file", { path });
  await refresh();
  if (wasOpen) await (back >= 0 ? open(nav.entry(back), { to: back }) : open(null));
  return content;
}

/** Undo for creating a page (or redo for deleting one): only while it still holds `expected`, so no work is lost. */
async function trashIfUnchanged(path, expected) {
  await flush();
  const n = note(path);
  if (!n) return; // already gone
  if (n.content !== expected) throw `${baseName(path)} has changed since, so it was kept`;
  await trash(path);
}

/** Puts a page back at its path; never overwrites one that's there now. */
async function restore(path, content) {
  await invoke("create_file", { path, content }); // fails with "<path> already exists."
  await refresh();
}

async function deletePage(path) {
  if (!path || !note(path) || modal()) return;
  if (!(await confirmDelete(path))) return;
  const name = baseName(path);
  try {
    const content = await trash(path);
    record({ label: `Delete ${name}`, undo: () => restore(path, content), redo: () => trashIfUnchanged(path, content) });
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
  n.content = written;
  if (path === current) {
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

// ---------- renaming ----------

let renaming = null; // the page the Rename dialog is for

function openRenameDialog(path) {
  if (!path || !note(path) || modal()) return;
  renaming = path;
  $("rename-name").value = baseName(path);
  $("rename-error").textContent = "";
  $("rename-dialog").showModal();
  $("rename-name").select();
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
  for (const { path, content } of [...vault.notes, ...templates]) {
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

$("rename-cancel").addEventListener("click", () => $("rename-dialog").close());
$("rename-form").addEventListener("submit", async (e) => {
  if (e.submitter?.value !== "rename") return;
  e.preventDefault();
  const from = renaming;
  const name = $("rename-name").value.trim();
  const problem = badName(name);
  if (problem) {
    $("rename-error").textContent = problem;
    return $("rename-name").focus();
  }
  const folder = from.split("/").slice(0, -1).join("/");
  const to = folder ? `${folder}/${name}.md` : `${name}.md`;
  if (to === from) return $("rename-dialog").close();
  let done;
  try {
    done = await renamePage(from, to);
  } catch (err) {
    return ($("rename-error").textContent = err);
  }
  $("rename-dialog").close();
  const { edits, failed } = done;
  // ponytail: undo / redo report only their label, so a page whose links couldn't be saved then goes unmentioned.
  record({ label: `Rename ${baseName(from)}`, undo: () => renamePage(to, from, edits.map(([p, was, now]) => [p, now, was])), redo: () => renamePage(from, to) });
  const n = edits.filter(([p]) => p !== to).length;
  say(failed.length ? `Renamed, but couldn't update the links in ${failed.join(", ")}`
    : `Renamed to ${name}${n ? `; links updated in ${n} page${n === 1 ? "" : "s"}` : ""}`);
});
$("rename").addEventListener("click", () => openRenameDialog(current));

// ---------- Obsidian ----------

let guidePath = null; // the page the guide was opened for
/** Opens `path` in Obsidian, or explains how to add the notes folder as a vault; `launch` also starts Obsidian. */
async function openInObsidian(path, launch = false) {
  let r;
  try {
    r = await invoke("open_in_obsidian", { path, launch });
  } catch (err) {
    return say(err);
  }
  if (!r.needsVault) return $("obsidian-dialog").open && $("obsidian-dialog").close();
  if ($("obsidian-dialog").open) return;
  guidePath = path;
  $("obsidian-path").textContent = r.path;
  $("obsidian-copied").textContent = "";
  $("obsidian-launch").hidden = !r.installed;
  $("obsidian-dialog").showModal();
}

$("obsidian").addEventListener("click", () => openInObsidian(current));
$("obsidian-launch").addEventListener("click", () => openInObsidian(guidePath, true));
$("obsidian-copy").addEventListener("click", () => {
  const text = $("obsidian-path").textContent;
  navigator.clipboard.writeText(text)
    .catch(() => invoke("plugin:clipboard-manager|write_text", { text }))
    .then(() => ($("obsidian-copied").textContent = "Copied"), (err) => ($("obsidian-copied").textContent = `Copy failed: ${err}`));
});

$("new-page").addEventListener("click", () => openNewDialog());
$("new-session").addEventListener("click", async () => {
  try {
    const path = await invoke("start_session");
    await refresh();
    const content = note(path)?.content ?? "";
    record({ label: `Start ${baseName(path)}`, undo: () => trashIfUnchanged(path, content), redo: () => restore(path, content) });
    open(path);
  } catch (err) {
    say(`Couldn't start a session: ${err}`);
  }
});

$("take-theirs").addEventListener("click", async () => {
  inConflict = false;
  $("conflict").hidden = true;
  await refresh();
  base = note(current)?.content ?? "";
  ed().setValue(base);
  render();
});
$("keep-mine").addEventListener("click", async () => {
  inConflict = false;
  $("conflict").hidden = true;
  const mine = ed().getValue();
  await loadVault();
  base = note(current)?.content ?? ""; // save over what's on disk now
  ed().setValue(mine); // a refresh during the await may have loaded theirs
  saving = save().finally(() => (saving = null));
});

// ---------- menu bar, shortcuts, context menu ----------

const zoomKey = (key) => () => window.dispatchEvent(new KeyboardEvent("keydown", { key, metaKey: mac, ctrlKey: !mac }));

/** What the menu bar and the keyboard shortcuts do; each reuses the button or function behind it. */
const actions = {
  search: () => $("search").focus(),
  toggle: () => current && $("toggle").click(),
  newPage: () => $("new-dialog").open || openNewDialog(),
  newSession: () => $("new-session").click(),
  obsidian: () => current && canObsidian() && $("obsidian").click(),
  settings: () => invoke("open_settings").catch(say),
  home: () => modal() || open(null),
  back: () => modal() || go(-1),
  forward: () => modal() || go(1),
  deletePage: () => deletePage(current),
  renamePage: () => openRenameDialog(current),
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
  else name = (e.shiftKey && { n: "newSession", h: "home" }[key]) || { k: "search", e: "toggle", n: "newPage", ",": "settings", "[": "back", "]": "forward" }[key];
  if (!name) return;
  e.preventDefault(); // also keeps the matching menu accelerator from firing on macOS
  run(name);
});

/** The native menu bar: app-wide on macOS (replacing Tauri's default one), the main window's own menu bar elsewhere. */
async function buildMenu() {
  const Menu = window.__TAURI__.menu?.Menu;
  if (!Menu) return;
  const sep = { item: "Separator" };
  const linuxOk = ["Separator", "Cut", "Copy", "Paste", "SelectAll"]; // GTK greys out the other predefined items
  const predefined = (...names) => names.filter((n) => !linux || linuxOk.includes(n)).map((item) => ({ item }));
  const item = (text, name, accelerator) => ({ text, accelerator, action: () => run(name, "menu") });
  const version = await window.__TAURI__.app?.getVersion().catch(() => undefined);
  const menu = await Menu.new({
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
        item("Open in Obsidian", "obsidian"),
        item("Rename…", "renamePage", "F2"),
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
    ],
  });
  await (mac ? menu.setAsAppMenu() : menu.setAsWindowMenu());
}

function copyLink(path) {
  const text = `[[${baseName(path)}]]`;
  // A native menu click isn't a user gesture, so WebKit may refuse; plain text, since copy_html also writes HTML, which Obsidian converts on paste.
  navigator.clipboard.writeText(text)
    .catch(() => invoke("plugin:clipboard-manager|write_text", { text }))
    .then(() => say(`Copied ${text}`), (err) => say(`Copy failed: ${err}`));
}

$("sidebar").addEventListener("contextmenu", (e) => {
  const Menu = window.__TAURI__.menu?.Menu;
  const file = e.target.closest(".file")?.dataset.path;
  const folder = e.target.closest("summary")?.parentElement.dataset.folder;
  if (!Menu || (file === undefined && folder === undefined)) return;
  e.preventDefault();
  const reveal = mac ? "Show Vault in Finder" : windows ? "Show Vault in Explorer" : "Open Vault Folder";
  const items = file !== undefined
    ? [
        { text: "Open", action: () => open(file) },
        ...(canObsidian() ? [{ text: "Open in Obsidian", action: () => openInObsidian(file) }] : []),
        { text: reveal, action: () => invoke("open_vault_folder").catch(say) },
        { item: "Separator" },
        { text: "Copy Link", action: () => copyLink(file) },
        { item: "Separator" },
        { text: "Rename…", action: () => openRenameDialog(file) },
        { text: "Delete…", action: () => deletePage(file) },
      ]
    : [{ text: "New Page Here…", action: () => openNewDialog("", { folder }) }];
  // ponytail: one small menu resource per right-click is never closed; call close() after popup if that ever matters.
  Menu.new({ items }).then((m) => m.popup()).catch(say);
});

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
  if (settings.sessionView !== prev.sessionView) sessionView = settings.sessionView;
  if (prev.vaultPath !== undefined && settings.vaultPath !== prev.vaultPath) refresh();
  else render();
  $("campaign").textContent = campaignName(settings.vaultPath);
}

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
  const Menu = window.__TAURI__.menu?.Menu;
  if (!Menu) return;
  const items = (settings.campaigns ?? []).map((path) =>
    ({ text: campaignName(path), checked: path === settings.vaultPath, action: () => switchCampaign(path) }));
  const manage = { text: "Add or Remove Campaigns…", action: () => run("settings", "menu") };
  Menu.new({ items: [...items, { item: "Separator" }, manage] }).then((m) => m.popup()).catch(say);
});
// From the tray menu and from Settings after adding a campaign.
listen("switch-campaign", (e) => switchCampaign(e.payload));

$("home").insertAdjacentHTML("afterbegin", icon("home"));
$("back").innerHTML = icon("back");
$("forward").innerHTML = icon("forward");
$("delete").innerHTML = icon("trash");
// Mouse back / forward buttons.
window.addEventListener("mouseup", (e) => {
  if (e.button !== 3 && e.button !== 4) return;
  e.preventDefault();
  run(e.button === 3 ? "back" : "forward", "mouse");
});

$("editor-hint").innerHTML = Object.entries(KINDS)
  .map(([kind, [k, label]]) => `<span>${escape(k)} ${icon(kind)}${label}</span>`).join(" · ");

// Hotkey notes and edits made in Obsidian show up here without a manual reload.
listen("vault-changed", refresh);
listen("settings-changed", (e) => applySettings(e.payload));
// The global New Page hotkey (Rust shows this window first).
listen("new-page", () => $("new-dialog").open || openNewDialog());
window.addEventListener("focus", refresh);
window.addEventListener("beforeunload", flush);

applySettings(await invoke("get_settings").catch(() => ({})));
await refresh();
open(null); // Home

buildMenu().catch((err) => console.error("menu:", err));
