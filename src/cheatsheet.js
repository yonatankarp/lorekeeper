// The cheat sheet (Help > Cheat Sheet, ⌘/ or Ctrl+/): note symbols, link syntax and keyboard shortcuts in one place.
// No DOM, tested in node. The note box placeholder and the session hints read NOTE_PREFIXES too, so they can't drift.
import { escape } from "./notes.js";

/** The quick-note symbols notes.js files notes by. The first key is the one hints show. */
export const NOTE_PREFIXES = [
  { keys: ["@"], kind: "npc", label: "NPCs", example: "@[[Mirela]] runs the inn" },
  { keys: ["#"], kind: "loot", label: "Loot", example: "#Silver dagger" },
  { keys: ["!"], kind: "quest", label: "Quests", example: "!Find the missing miners" },
  { keys: ["?"], kind: "mystery", label: "Mysteries", example: "?Who sent the letter" },
  { keys: ['"', "“"], kind: "quote", label: "Quotes", example: '"Run!" - Vex' },
];

/** A quick note starting with this is private (privateNote in notes.js, private_note in lib.rs). */
export const PRIVATE_PREFIX = "~";

// ---------- shortcuts: "CmdOrCtrl+Alt+N" shows as ⌘⌥N on macOS, Ctrl+Alt+N elsewhere ----------

const MAC_MODS = { cmdorctrl: "⌘", cmd: "⌘", command: "⌘", super: "⌘", ctrl: "⌃", control: "⌃", alt: "⌥", option: "⌥", shift: "⇧" };
const OTHER_MODS = { cmdorctrl: "Ctrl", ctrl: "Ctrl", control: "Ctrl", super: "Win", cmd: "Win", command: "Win", alt: "Alt", option: "Alt", shift: "Shift" };
const KEYS = { ArrowUp: "↑", ArrowDown: "↓", ArrowLeft: "←", ArrowRight: "→", Comma: ",", Period: ".", Slash: "/", Backslash: "\\",
  Semicolon: ";", Quote: "'", BracketLeft: "[", BracketRight: "]", Backquote: "`", Minus: "-", Equal: "=", Plus: "+", Escape: "Esc" };

/** A Tauri accelerator ("CmdOrCtrl+Alt+N") the way this platform writes it. */
export function readable(accel, mac) {
  const mods = mac ? MAC_MODS : OTHER_MODS;
  return accel.split("+").map((part) => {
    const mod = part.toLowerCase().replace(/^(command|cmd)or(control|ctrl)$/, "cmdorctrl");
    const key = part.replace(/^(Key|Digit)(?=.)/, "");
    if (key === "Backspace" && mac) return "⌫";
    return mods[mod] ?? KEYS[key] ?? key;
  }).join(mac ? "" : "+");
}

// The global shortcuts' defaults (Settings::default in lib.rs), for before the settings have loaded.
const QUICK_NOTE = "CmdOrCtrl+Alt+N";
const SAVE_SELECTION = "CmdOrCtrl+Shift+S";

/** The Quick note and Save selection shortcuts as set, for hints. */
export const globalKeys = (settings = {}) => ({ quickNote: settings.quickNote || QUICK_NOTE, capture: settings.capture || SAVE_SELECTION });

// ---------- the sheet ----------

const code = (text) => `<code>${escape(text)}</code>`;
const kbd = (keys, mac) => keys.map((k) => `<kbd>${escape(readable(k, mac))}</kbd>`).join(" or ");

/** One group: a heading and a two-column table, [keyHtml, whatHtml] rows. */
const group = (id, title, head, rows, note = "") =>
  `<section aria-labelledby="cheat-${id}"><h3 id="cheat-${id}">${title}</h3>` +
  `<table class="cheat-table"><thead><tr><th scope="col">${head}</th><th scope="col">What it does</th></tr></thead><tbody>` +
  rows.map(([key, what]) => `<tr><th scope="row">${key}</th><td>${what}</td></tr>`).join("") +
  `</tbody></table>${note ? `<p class="cheat-note">${note}</p>` : ""}</section>`;

/**
 * The cheat sheet as HTML. `settings` are the app's (the global shortcuts as the user set them); `mac` picks ⌘ or Ctrl.
 * Everything from settings is escaped.
 */
export function cheatSheetHtml({ settings = {}, mac = false } = {}) {
  const k = (...keys) => kbd(keys, mac);
  const { quickNote, capture } = globalKeys(settings);
  const optional = (accel) => (accel ? k(accel) : "Off");

  const prefixes = group("notes", "Note symbols", "Start with", [
    ...NOTE_PREFIXES.map(({ keys, label, example }) => [
      keys.map((key) => `<kbd>${escape(key)}</kbd>`).join(" or "),
      `<strong>${label}</strong>: ${code(example)}${keys.length > 1 ? ". Quotes keep their marks." : ""}`,
    ]),
    ["No symbol", "<strong>What happened</strong>"],
    [`<kbd>${PRIVATE_PREFIX}</kbd>`, `<strong>Private</strong>, in the note box of a shared campaign: the note goes to your private notes for the session, still filed by the symbol after it (${code("~@[[Halia]] lies")}). The box says who reads it. In a campaign of your own the ${code(PRIVATE_PREFIX)} is dropped and the note is saved as usual.`],
  ], `Works in the note box and at the start of any line on a session page. Notes show with their kind in the <strong>Timeline</strong> and grouped in the <strong>Journal</strong>. On a session page, write loot as a list item (${code("- #Silver dagger")}): a line starting with ${code("#")} is read as a heading and doesn't show as a note.`);

  const links = group("links", "Links and images", "Type", [
    [code("[[Mirela]]"), "A link to the page named Mirela, in any folder. Clicking a link to a page that doesn't exist yet opens <strong>New page</strong> to make it."],
    [code("[[Baron Vex|the Baron]]"), "A link to Baron Vex that reads \"the Baron\"."],
    [code("[[NPCs/Vex]]"), "A link to the Vex in NPCs, when two pages share a name."],
    [code("@[[Mirela]]"), `A mention: the ${code("@")} files the note under NPCs, the link lists it on Mirela's page under <strong>Linked from</strong>. ${code("@Mirela")} without brackets is not a link.`],
    [`${code("@Mir")} or ${code("[[Mir")}`, `Suggests page names. ${k("Tab")} picks one and makes the link.`],
    [code("![[map.png]]"), `Shows an image from the notes folder. ${code("![[map.png|300]]")} sets its width, ${code("![[map.png|300x200]]")} width and height. Paste or drop an image into the editor to save it in Attachments and embed it.`],
    [code("![Map](Maps/map.png)"), "A Markdown image, which works too."],
    [code("[Site](https://example.com)"), "A web link: it opens in your browser."],
  ]);

  const anywhere = group("global", "Anywhere", "Keys", [
    [k(quickNote), "<strong>Quick note</strong>: a one-line note box, over any app"],
    [k(capture), "<strong>Save selection</strong>: saves the selected text, or the clipboard"],
    [optional(settings.newSession), "<strong>New session</strong>, without opening a window"],
    [optional(settings.newPage), "<strong>New page</strong>: opens the New page dialog"],
  ], "These work even while Lorekeeper is in the background. Change them in <strong>Settings &gt; Shortcuts</strong>.");

  const noteBox = group("note-box", "In the note box", "Keys", [
    [k("Enter"), "Saves the note"],
    [k("CmdOrCtrl+Enter"), "Saves it to a new session, when the box offers one (the last note is over 12 hours old)"],
    [`${k("ArrowUp")} in the empty box`, "Brings back the last note to fix: Enter saves the fix in place, Esc cancels"],
    [k("Tab"), "Takes the suggested page name"],
    [`${k("ArrowUp")} ${k("ArrowDown")}`, "Picks another suggestion"],
    [k("Escape"), "Drops the suggestion, then closes the box without saving"],
  ]);

  const app = group("app", "In Lorekeeper", "Keys", [
    [k("CmdOrCtrl+K"), "Search"],
    [k("CmdOrCtrl+E"), "Edit / Preview"],
    [k("CmdOrCtrl+N"), "New page"],
    [k("CmdOrCtrl+Shift+N"), "New session"],
    [k("CmdOrCtrl+Shift+H"), "Home"],
    [`${k("CmdOrCtrl+[")} ${k("CmdOrCtrl+]")}`, "Back, Forward"],
    [k("F2"), "Rename the page, or give a session a title"],
    [k("CmdOrCtrl+Backspace"), "Move the page to the Trash (outside text)"],
    [k("CmdOrCtrl+Z"), "Undo, including deleting, renaming and moving pages"],
    [mac ? k("CmdOrCtrl+Shift+Z") : k("CmdOrCtrl+Shift+Z", "Ctrl+Y"), "Redo"],
    [k("CmdOrCtrl+,"), "Settings"],
    [`${k("CmdOrCtrl+Plus")} ${k("CmdOrCtrl+Minus")} ${k("CmdOrCtrl+0")}`, "Zoom in, zoom out, actual size"],
    [k("CmdOrCtrl+/"), "This cheat sheet"],
    [k("Escape"), "Closes a dialog"],
  ]);

  const editor = group("editor", "In the editor", "Keys", [
    [k("CmdOrCtrl+B"), "Bold"],
    [k("CmdOrCtrl+I"), "Italic"],
    [`${k("Tab")} or ${k("Enter")}`, `Takes the suggested page name (type ${code("[[")} or ${code("@")} for suggestions)`],
    [`${k("Tab")} ${k("Shift+Tab")}`, "Indents or outdents a list item"],
    [k("Enter"), "Continues a list"],
    [k("CmdOrCtrl+Click"), "Follows a link (a plain click just places the cursor)"],
  ]);

  const search = group("search", "In search", "Keys", [
    [`${k("ArrowDown")} ${k("ArrowUp")}`, "Moves through the results"],
    [k("Enter"), "Opens the result"],
    [k("Escape"), "Clears the search"],
  ]);

  return prefixes + links + anywhere + noteBox + app + editor + search;
}
