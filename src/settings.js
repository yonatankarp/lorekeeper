import { applyTheme } from "./theme.js";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);
const isMac = navigator.userAgent.includes("Mac");
let current = null, recording = null, saves = Promise.resolve(), statusTimer;

// ---------- shortcuts: "CmdOrCtrl+Alt+N" shows as ⌘⌥N on macOS, Ctrl+Alt+N elsewhere ----------

const MODS = isMac
  ? { cmdorctrl: "⌘", cmd: "⌘", command: "⌘", super: "⌘", ctrl: "⌃", control: "⌃", alt: "⌥", option: "⌥", shift: "⇧" }
  : { cmdorctrl: "Ctrl", ctrl: "Ctrl", control: "Ctrl", super: "Win", cmd: "Win", command: "Win", alt: "Alt", option: "Alt", shift: "Shift" };
const KEYS = { ArrowUp: "↑", ArrowDown: "↓", ArrowLeft: "←", ArrowRight: "→", Comma: ",", Period: ".", Slash: "/", Backslash: "\\",
  Semicolon: ";", Quote: "'", BracketLeft: "[", BracketRight: "]", Backquote: "`", Minus: "-", Equal: "=" };

function readable(accel) {
  return accel.split("+").map((part) => {
    const mod = part.toLowerCase().replace(/^(command|cmd)or(control|ctrl)$/, "cmdorctrl");
    const key = part.replace(/^(Key|Digit)(?=.)/, "");
    return MODS[mod] ?? KEYS[key] ?? key;
  }).join(isMac ? "" : "+");
}

// A keydown as a Tauri accelerator. e.code, not e.key, so Option+N on a Mac is N and not "˜".
function accelerator(e) {
  const mods = [];
  if (isMac ? e.metaKey : e.ctrlKey) mods.push("CmdOrCtrl");
  if (isMac && e.ctrlKey) mods.push("Ctrl");
  if (!isMac && e.metaKey) mods.push("Super");
  if (e.altKey) mods.push("Alt");
  // Shift alone would take over typing capitals everywhere.
  if (!mods.length) return null;
  if (e.shiftKey) mods.push("Shift");
  return [...mods, e.code.replace(/^(Key|Digit)/, "")].join("+");
}

function stopRecording() {
  if (!recording) return;
  recording.setAttribute("aria-pressed", "false");
  $(`${recording.dataset.record}-keys`).classList.remove("recording");
  recording = null;
  render(current);
}

for (const button of document.querySelectorAll("[data-record]")) {
  button.setAttribute("aria-pressed", "false");
  button.addEventListener("click", () => {
    const again = recording === button;
    stopRecording();
    if (again) return;
    recording = button;
    button.setAttribute("aria-pressed", "true");
    const keys = $(`${button.dataset.record}-keys`);
    keys.textContent = "Press keys…";
    keys.classList.add("recording");
  });
  button.addEventListener("blur", stopRecording);
}

document.addEventListener("keydown", (e) => {
  if (!recording) return;
  const plain = !e.metaKey && !e.ctrlKey && !e.altKey && !e.shiftKey;
  if (e.key === "Tab" && plain) return stopRecording(); // let focus move on
  e.preventDefault();
  e.stopPropagation();
  if (e.key === "Escape") return stopRecording();
  if (/^(Meta|OS|Control|Alt|Shift)(Left|Right)?$|^(CapsLock|Fn)$/.test(e.code)) return; // wait for the key
  const key = recording.dataset.record;
  const accel = accelerator(e);
  if (!accel) {
    $(`${key}-error`).textContent = `Hold ${isMac ? "⌘, ⌥ or ⌃" : "Ctrl or Alt"} while you press the key.`;
    return;
  }
  stopRecording();
  save({ [key]: accel });
}, true);

// ---------- load, show and save ----------

function render(s) {
  if (!s) return;
  current = s;
  applyTheme(s.theme);
  for (const key of ["quickNote", "capture"]) {
    if (recording?.dataset.record !== key) $(`${key}-keys`).textContent = readable(s[key]);
  }
  $("vaultPath").textContent = s.vaultPath;
  for (const el of document.querySelectorAll("[data-setting]")) {
    const value = s[el.dataset.setting];
    if (el.type === "checkbox") el.checked = value;
    else el.value = value;
  }
  $("editorFontSize-value").textContent = `${s.editorFontSize} px`;
}

// One save at a time, each applied on top of the last result, so quick changes don't undo each other.
function save(changes) {
  const error = $(`${Object.keys(changes)[0]}-error`);
  error.textContent = "";
  saves = saves.then(async () => {
    try {
      render(await invoke("save_settings", { settings: { ...current, ...changes } }));
      clearTimeout(statusTimer);
      $("saved-status").textContent = "Saved";
      statusTimer = setTimeout(() => { $("saved-status").textContent = ""; }, 1500);
    } catch (err) {
      error.textContent = String(err);
      render(current); // put the control back
    }
  });
}

for (const el of document.querySelectorAll("[data-setting]")) {
  el.addEventListener("change", () => {
    const value = el.type === "checkbox" ? el.checked : el.type === "range" ? Number(el.value) : el.value;
    save({ [el.dataset.setting]: value });
  });
}
$("editorFontSize").addEventListener("input", (e) => { $("editorFontSize-value").textContent = `${e.target.value} px`; });

$("choose").addEventListener("click", async () => {
  const path = await invoke("pick_vault_folder");
  if (path) save({ vaultPath: path });
});
$("reveal").addEventListener("click", () => invoke("open_vault_folder"));

const fileManager = isMac ? "Finder" : navigator.userAgent.includes("Windows") ? "Explorer" : "your file manager";
for (const el of document.querySelectorAll(".file-manager")) el.textContent = fileManager;
for (const el of document.querySelectorAll(".primary-key")) el.textContent = isMac ? "⌘" : "Ctrl";

// Queued behind saves, so the focus after the folder picker can't show (and later re-save) the old folder.
const load = () => { saves = saves.then(() => invoke("get_settings").then(render)); };
load();
window.addEventListener("focus", load);
listen("settings-changed", (e) => render(e.payload));
