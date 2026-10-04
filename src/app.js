// The Lorekeeper window: browse the vault, read and edit pages, follow [[links]], search, backlinks.
import { marked } from "./vendor/marked.esm.js";
import { createEditor } from "./editor.js";
import { escape, parse, timeline, toHtml, toText } from "./notes.js";
import { backlinks, badName, baseName, buildTree, fillTemplate, folderFor, resolve, search, splitFrontmatter } from "./vault.js";
import { applyTheme } from "./theme.js";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);

let vault = { folders: [], notes: [], currentSession: "" };
let templates = []; // Templates/ notes: used by "New page", hidden everywhere else
let current = null; // open page path
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
const dirty = () => saveTimer !== null || saving !== null;
const today = () => new Date().toLocaleDateString("sv-SE"); // YYYY-MM-DD
const unescape = (s) => s.replace(/&(amp|lt|gt|quot|#39);/g, (_, e) => ({ amp: "&", lt: "<", gt: ">", quot: '"', "#39": "'" })[e]);

/** The Markdown editor, created the first time it's needed. */
function ed() {
  if (!editor) {
    editor = createEditor($("editor"), {
      onChange: () => {
        scheduleSave();
        syncCopy();
      },
      onFollowLink: followLink,
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

marked.use({
  gfm: true,
  breaks: true, // Obsidian shows single newlines as line breaks
  renderer: { html: ({ text }) => escape(text) }, // raw HTML in notes is shown, not run
  extensions: [
    {
      name: "wikilink",
      level: "inline",
      start: (src) => src.match(/!?\[\[/)?.index,
      tokenizer(src) {
        const m = /^!?\[\[([^\]|#]*)(#[^\]|]*)?(?:\|([^\]]*))?\]\]/.exec(src);
        if (m) return { type: "wikilink", raw: m[0], target: m[1].trim(), label: (m[3] ?? m[1]).trim() };
      },
      renderer: (t) => linkHtml(t.target, escape(t.label)),
    },
  ],
});

const inlineLinks = (escapedText) =>
  escapedText.replace(/!?\[\[([^\]|#]*)(#[^\]|]*)?(?:\|([^\]]*))?\]\]/g, (_, target, heading, label) =>
    linkHtml(unescape(target.trim()), (label ?? target).trim()),
  );

function propsHtml(props) {
  if (!props.length) return "";
  const rows = props.map(([k, v]) => `<dt>${escape(k)}</dt><dd>${inlineLinks(escape(v.replace(/^["']|["']$/g, "")))}</dd>`);
  return `<dl class="props">${rows.join("")}</dl>`;
}

// ---------- sidebar ----------

function treeHtml(node) {
  const dirs = node.dirs
    .map((d) => {
      const n = d.files.length;
      const count = n ? `<span class="sr-only">, </span>${n}<span class="sr-only"> page${n === 1 ? "" : "s"}</span>` : "";
      const tpl = templates.map((t) => baseName(t.path)).find((t) => folderFor(t, vault.folders) === d.path) ?? "";
      const empty = `<button type="button" class="folder-new" data-template="${escape(tpl)}" data-folder="${escape(d.path)}">+ New ${escape(tpl || "page")}</button>`;
      return `<details data-folder="${escape(d.path)}"${closedFolders.has(d.path) ? "" : " open"}>
        <summary>${escape(d.name)}<span class="count">${count}</span></summary>
        <div class="children">${treeHtml(d) || empty}</div></details>`;
    })
    .join("");
  const files = node.files
    .map((p) => `<button class="file${p === current ? " active" : ""}"${p === current ? ' aria-current="page"' : ""} data-path="${escape(p)}" title="${escape(p)}">${escape(baseName(p))}</button>`)
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
  `<dl class="prefix-legend">${Object.values(KINDS).map(([k, label]) => `<dt><kbd>${escape(k)}</kbd></dt><dd>${label}</dd>`).join("")}</dl>`;

const emptySessionHtml = `<div class="empty-state"><h2>No notes yet</h2>
  <p>Press <kbd>⌘⌥N</kbd> for a quick note, or <kbd>⌘⇧S</kbd> to save selected text (or the clipboard).</p>
  <p>Notes appear here live during the game. Start a note with a symbol to file it:</p>${legendHtml}</div>`;

const timelineHtml = (items) =>
  `<ol class="timeline">${items
    .map(({ time, kind, text }) => {
      const badge = KINDS[kind] ? `<span class="kind kind-${kind}">${KINDS[kind][1]}</span>` : "";
      return `<li><time>${escape(time)}</time>${badge}<span class="text">${inlineLinks(escape(text))}</span></li>`;
    })
    .join("")}</ol>`;

function sessionHtml(content) {
  const session = parse(content);
  const items = timeline(content);
  const title = `<h1>${session.title ? inlineLinks(escape(session.title)) : escape(baseName(current))}</h1>`;
  if (!items.length) return title + emptySessionHtml;
  const pressed = (v) => `aria-pressed="${sessionView === v}"`;
  const views = `<div class="view-switch" role="group" aria-label="Session view">
    <button type="button" data-view="timeline" ${pressed("timeline")}>Timeline</button>
    <button type="button" data-view="journal" ${pressed("journal")}>Journal</button></div>`;
  return views + title + (sessionView === "journal"
    ? toHtml({ ...session, title: "" }, (t, label) => linkHtml(unescape(t.trim()), label)) +
      `<p class="session-note">This is what “Copy for D&amp;D Beyond” pastes. Click Edit to see every line.</p>`
    : timelineHtml(items));
}

/** Copy is pointless until the session has a note; while editing, the unsaved text counts. */
const syncCopy = () =>
  ($("copy").disabled = !timeline(editing ? ed().getValue() : note(current)?.content ?? "").length);

function render() {
  const n = note(current);
  const folder = current ? current.split("/").slice(0, -1).join(" / ") : "";
  $("crumbs").innerHTML = n ? `${folder ? `<span class="folder">${escape(folder)} / </span>` : ""}<strong>${escape(baseName(current))}</strong>` : "";
  $("toggle").hidden = !n;
  $("obsidian").hidden = !n || !vault.hasObsidian;
  $("copy").hidden = !n || !isSession(current);
  syncCopy();
  $("toggle").textContent = editing ? "Done" : "Edit";
  $("editor").hidden = !editing || !n;
  $("editor-hint").hidden = !editing || !n || !isSession(current);
  $("view").hidden = editing && !!n;

  if (!n) {
    $("view").innerHTML = `<div class="welcome"><p>No page open.</p>
      <p>Press <kbd>⌘⌥N</kbd> during a game to jot a note, or create a page with <kbd>+ New page</kbd>.</p></div>`;
    $("backlinks").innerHTML = "";
    return;
  }
  if (!editing) {
    const { props, body } = splitFrontmatter(n.content);
    $("view").innerHTML = isSession(current) ? sessionHtml(n.content) : propsHtml(props) + marked.parse(body);
  }
  const links = backlinks(current, vault.notes);
  $("backlinks").innerHTML = `<h2>Linked from</h2>` + (links.length
    ? `<ul>${links
        .map((b) => `<li><a data-path="${escape(b.path)}" href="#">${escape(baseName(b.path))}</a>
          ${b.lines.map((l) => `<div class="snip">${inlineLinks(escape(l.replace(/^[-*+] (\d{1,2}:\d{2} )?/, "")))}</div>`).join("")}</li>`)
        .join("")}</ul>`
    : `<p class="none">No pages link here yet. Link to it with [[${escape(baseName(current))}]].</p>`);
}

async function open(path, { edit = false } = {}) {
  await flush();
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

function followLink(target) {
  const path = resolve(target, paths());
  if (path) open(path);
  else openNewDialog(target.split("/").pop());
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

/** `template` / `folder` preselect the dialog (e.g. from an empty folder); otherwise it guesses from the open page. */
function openNewDialog(name = "", { template, folder } = {}) {
  const names = templates.map((n) => baseName(n.path));
  const folderOfCurrent = current?.includes("/") ? current.split("/").slice(0, -1).join("/") : "";
  const guess = template ?? names.find((t) => folderFor(t, vault.folders) === folderOfCurrent && folderOfCurrent) ?? "";
  $("new-template").innerHTML = `<option value="">Blank</option>` +
    names.map((t) => `<option${t === guess ? " selected" : ""}>${escape(t)}</option>`).join("");
  $("new-folder").innerHTML = `<option value="">(vault root)</option>` +
    vault.folders.map((f) => `<option>${escape(f)}</option>`).join("");
  $("new-folder").value = folder ?? (guess ? folderFor(guess, vault.folders) : (isSession(current) || !folderOfCurrent ? "" : folderOfCurrent));
  $("new-name").value = name;
  $("new-error").textContent = "";
  $("new-dialog").showModal();
  $("new-name").focus();
}

$("new-template").addEventListener("change", () => {
  const t = $("new-template").value;
  if (t) $("new-folder").value = folderFor(t, vault.folders);
});

$("new-form").addEventListener("submit", async (e) => {
  if (e.submitter?.value !== "create") return;
  e.preventDefault();
  const name = $("new-name").value.trim();
  const problem = badName(name);
  if (problem) return ($("new-error").textContent = problem);
  const folder = $("new-folder").value;
  const template = $("new-template").value;
  const path = folder ? `${folder}/${name}.md` : `${name}.md`;
  const tpl = template && templates.find((n) => n.path === `Templates/${template}.md`);
  const content = tpl ? fillTemplate(tpl.content, name, today()) : `# ${name}\n\n`;
  try {
    await invoke("create_file", { path, content });
  } catch (err) {
    return ($("new-error").textContent = err);
  }
  $("new-dialog").close();
  await refresh();
  open(path, { edit: true });
});

// ---------- events ----------

document.addEventListener("click", (e) => {
  const el = e.target.closest("a, .file");
  if (!el) return;
  e.preventDefault();
  if (el.dataset.path) return open(el.dataset.path);
  if (el.dataset.target !== undefined) return followLink(el.dataset.target);
  const href = el.getAttribute("href") ?? "";
  if (/^https?:/i.test(href)) invoke("open_url", { url: href }).catch(say);
  else if (href && !href.startsWith("#") && !/^[a-z][a-z0-9+.-]*:/i.test(href)) followLink(decodeURIComponent(href));
});

$("tree").addEventListener("click", (e) => {
  const b = e.target.closest(".folder-new");
  if (b) openNewDialog("", { template: b.dataset.template, folder: b.dataset.folder });
});

$("view").addEventListener("click", (e) => {
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

$("copy").addEventListener("click", async () => {
  const session = parse(editing ? ed().getValue() : note(current)?.content ?? "");
  await invoke("copy_html", { html: toHtml(session), text: toText(session) })
    .then(() => say("Copied. Paste it into the D&D Beyond journal"), (err) => say(`Copy failed: ${err}`));
});

$("obsidian").addEventListener("click", () => invoke("open_in_obsidian", { path: current }).catch(say));
$("new-page").addEventListener("click", () => openNewDialog());
$("new-session").addEventListener("click", async () => {
  try {
    const path = await invoke("start_session");
    await refresh();
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

document.addEventListener("keydown", (e) => {
  const mod = e.metaKey || e.ctrlKey;
  if (!mod) return;
  const key = e.key.toLowerCase();
  if (key === "k") $("search").focus();
  else if (key === "e" && current) $("toggle").click();
  else if (key === "n" && !$("new-dialog").open) openNewDialog();
  else if (key === ",") invoke("open_settings").catch(say);
  else return;
  e.preventDefault();
});

/** Theme, editor font size and the default session view from the Settings window. */
function applySettings(next) {
  const prev = settings;
  settings = { ...settings, ...next };
  applyTheme(settings.theme);
  editor?.setFontSize(settings.editorFontSize);
  if (settings.sessionView !== prev.sessionView) sessionView = settings.sessionView;
  if (prev.vaultPath !== undefined && settings.vaultPath !== prev.vaultPath) refresh();
  else render();
}

// Hotkey notes and edits made in Obsidian show up here without a manual reload.
listen("vault-changed", refresh);
listen("settings-changed", (e) => applySettings(e.payload));
window.addEventListener("focus", refresh);
window.addEventListener("beforeunload", flush);

applySettings(await invoke("get_settings").catch(() => ({})));
await refresh();
open(vault.currentSession || null);
