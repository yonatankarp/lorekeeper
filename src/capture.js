import { applyTheme } from "./theme.js";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const input = document.getElementById("note");
const ghost = document.getElementById("ghost");
const typed = document.getElementById("typed");
const rest = document.getElementById("rest");
let names = [], query = null, matches = [], pick = 0, saving = false;

const wait = (ms) => new Promise((r) => setTimeout(r, ms));
const dismiss = () => { input.value = ""; suggest(); invoke("dismiss", { restoreFocus: true }); };

// A page name being typed at the end of the box: "@Mir" or an unclosed "[[Mir".
// ponytail: end of the box only, since a ghost mid-text would overlap what follows.
function nameQuery() {
  const v = input.value;
  if (input.selectionStart !== v.length) return null;
  const m = v.match(/\[\[([^[\]]+)$/) || v.match(/(?:^|\s)@([^\s@[\]]+)$/);
  return m && { q: m[1], link: m[0].includes("[[") };
}

// Prefix matches first, then names that merely contain the query.
function suggest() {
  query = nameQuery();
  const q = query?.q.toLowerCase();
  const starts = (n) => n.toLowerCase().startsWith(q);
  matches = !query ? [] : [...names.filter(starts), ...names.filter((n) => !starts(n) && n.toLowerCase().includes(q))];
  pick = 0;
  render();
}

// Draws the rest of the picked name in grey right after the (invisible) typed text.
function render() {
  const name = matches[pick], q = query?.q ?? "";
  typed.textContent = name ? input.value : "";
  rest.textContent = !name ? ""
    : name.toLowerCase().startsWith(q.toLowerCase()) ? name.slice(q.length) || "]]" : `  → ${name}`;
  // ponytail: once a long note scrolls, only the part of the ghost that fits after the caret shows.
  ghost.scrollLeft = input.scrollLeft;
}

// "@Mir" → "@[[Mirela]]", "[[Mir" → "[[Mirela]]".
function accept() {
  const v = input.value;
  input.value = v.slice(0, v.length - query.q.length) + (query.link ? "" : "[[") + matches[pick] + "]]";
  input.setSelectionRange(input.value.length, input.value.length);
  suggest();
}

// Shows where the note went before closing; on failure the draft comes back so nothing is lost.
async function save(startNew = false) {
  const draft = input.value;
  saving = input.readOnly = true;
  matches = [];
  render();
  try {
    input.value = `✓ Saved to ${await invoke("save_note", { text: draft, startNew })}`;
    await wait(700);
    dismiss();
  } catch (err) {
    input.value = `✗ ${err}`;
    await wait(2000);
    input.value = draft;
  }
  saving = input.readOnly = false;
}

input.addEventListener("keydown", (e) => {
  if (e.isComposing || saving) return;
  if (matches.length && (e.key === "Tab" || e.key === "ArrowUp" || e.key === "ArrowDown")) {
    e.preventDefault();
    if (e.key === "Tab") accept();
    else {
      pick = (pick + (e.key === "ArrowDown" ? 1 : matches.length - 1)) % matches.length;
      render();
    }
  } else if (e.key === "Escape") {
    // First Escape drops a showing suggestion, the next one closes the box.
    if (matches.length) { matches = []; render(); } else dismiss();
  } else if (e.key === "Enter") {
    if (input.value.trim()) save((e.metaKey || e.ctrlKey) && document.body.classList.contains("stale")); else dismiss();
  }
});
// The macOS menu bar is app-wide, and its Undo / Redo items act on the main window: undo typing here instead,
// and keep the keys from reaching that menu (a handled keydown stops its accelerator).
document.addEventListener("keydown", (e) => {
  if (!(e.metaKey || e.ctrlKey) || e.key.toLowerCase() !== "z") return;
  e.preventDefault();
  document.execCommand(e.shiftKey ? "redo" : "undo");
});
input.addEventListener("input", suggest);
input.addEventListener("click", suggest);
input.addEventListener("keyup", (e) => { if (["ArrowLeft", "ArrowRight", "Home", "End"].includes(e.key)) suggest(); });
window.addEventListener("focus", () => {
  input.focus();
  invoke("page_names").then((n) => { names = n; suggest(); });
});
// Clicking away hides the box but keeps the draft for next time.
window.addEventListener("blur", () => invoke("dismiss", { restoreFocus: false }));

// Offers a new session when the current one has gone quiet for 12+ hours (session_status); Enter still saves to it.
const stale = document.getElementById("stale");
const mod = navigator.userAgent.includes("Mac") ? "⌘" : "Ctrl+";
const ago = (h) => (h < 48 ? `${h} hours` : `${Math.floor(h / 24)} days`);
window.addEventListener("focus", () => invoke("session_status").then((s) => {
  document.body.classList.toggle("stale", !!s);
  if (s) stale.textContent = `Last note was ${ago(s.idleHours)} ago. ${mod}Enter saves to a new Session ${s.next}.`;
}));

invoke("get_settings").then((s) => applyTheme(s.theme));
listen("settings-changed", (e) => applyTheme(e.payload.theme));
