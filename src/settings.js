import { applyTheme } from "./theme.js";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);
const isMac = navigator.userAgent.includes("Mac");
let current = null, recording = null, saves = Promise.resolve(), statusTimer;
let backup = null, signingIn = false, signInAttempt = 0;

// ---------- platform and tabs ----------

// Picks the tab bar style in settings.css. Left alone if something set it first.
const platform = (navigator.userAgentData?.platform || navigator.platform || "").toLowerCase();
document.documentElement.dataset.platform ||= platform.startsWith("mac") ? "macos" : platform.startsWith("win") ? "windows" : "linux";

const tabs = [...document.querySelectorAll('[role="tab"]')];
function selectTab(tab, focus) {
  for (const t of tabs) {
    t.setAttribute("aria-selected", t === tab);
    t.tabIndex = t === tab ? 0 : -1;
    $(t.getAttribute("aria-controls")).hidden = t !== tab;
  }
  if (focus) tab.focus();
  try { localStorage.setItem("settings-tab", tab.id); } catch {}
}
for (const tab of tabs) tab.addEventListener("click", () => selectTab(tab));
$("tabs").addEventListener("keydown", (e) => {
  const i = tabs.indexOf(e.target);
  const next = { ArrowLeft: i - 1, ArrowRight: i + 1, Home: 0, End: tabs.length - 1 }[e.key];
  if (i < 0 || next === undefined) return;
  e.preventDefault();
  selectTab(tabs[(next + tabs.length) % tabs.length], true);
});
// Bubble phase: while a shortcut is being recorded, the recorder's capture listener takes the keys first.
document.addEventListener("keydown", (e) => {
  if (!(isMac ? e.metaKey : e.ctrlKey) || e.altKey || e.shiftKey || !/^Digit[1-4]$/.test(e.code)) return;
  e.preventDefault();
  selectTab(tabs[e.code.slice(-1) - 1], true);
});
let savedTab = null;
try { savedTab = localStorage.getItem("settings-tab"); } catch {}
selectTab(tabs.find((t) => t.id === savedTab) ?? tabs[0]);

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
  $("backupFolder").textContent = s.backupFolder || "Off";
  $("backup-off").disabled = !s.backupFolder;
  $("gh-login").textContent = `@${s.githubUser}`;
  $("gh-repo").textContent = `${s.githubUser}/${s.githubRepo}`;
  showGithub();
  renderBackup(backup);
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
  const path = await invoke("pick_folder", { title: "Choose a notes folder", start: current.vaultPath });
  if (path) save({ vaultPath: path });
});
$("reveal").addEventListener("click", () => invoke("open_vault_folder"));

const fileManager = isMac ? "Finder" : navigator.userAgent.includes("Windows") ? "Explorer" : "your file manager";
for (const el of document.querySelectorAll(".file-manager")) el.textContent = fileManager;
for (const el of document.querySelectorAll(".primary-key")) el.textContent = isMac ? "⌘" : "Ctrl";

// ---------- backups ----------

// "today 21:30", "yesterday 21:30", or the date.
function when(iso) {
  const d = new Date(iso), day = (x) => new Date(x).setHours(0, 0, 0, 0);
  const time = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  const days = Math.round((day(Date.now()) - day(d)) / 86400000);
  return days === 0 ? `today ${time}` : days === 1 ? `yesterday ${time}` : `${d.toLocaleDateString()} ${time}`;
}

function targetStatus(t, on) {
  if (!on || !t) return "";
  if (backup.running) return "Backing up…";
  const last = t.lastOk ? `Last backup: ${when(t.lastOk)}` : "No backup yet.";
  return t.warning ? `${last}. ${t.warning}` : last;
}

function renderBackup(status) {
  if (status) backup = status;
  if (!backup || !current) return;
  status = backup;
  $("folder-status").textContent = targetStatus(status.folder, current.backupFolder);
  $("folder-run-error").textContent = current.backupFolder ? status.folder.lastError : "";
  $("github-status").textContent = targetStatus(status.github, current.githubUser);
  $("github-run-error").textContent = current.githubUser ? status.github.lastError : "";
  $("backup-now").disabled = $("gh-now").disabled = status.running;
  $("backup-now").disabled ||= !current.backupFolder;
  $("gh-unavailable").hidden = status.githubAvailable;
  $("gh-sign-in").disabled = !status.githubAvailable;
}

function showGithub() {
  $("gh-signed-out").hidden = signingIn || !!current.githubUser;
  $("gh-flow").hidden = !signingIn;
  $("gh-signed-in").hidden = signingIn || !current.githubUser;
}

$("backup-choose").addEventListener("click", async () => {
  const path = await invoke("pick_folder", { title: "Choose a backup folder", start: current.backupFolder });
  if (path) save({ backupFolder: path });
});
$("backup-off").addEventListener("click", () => save({ backupFolder: "" }));
for (const id of ["backup-now", "gh-now"]) $(id).addEventListener("click", () => invoke("backup_now"));
$("gh-repo").addEventListener("click", (e) => {
  e.preventDefault();
  invoke("open_url", { url: `https://github.com/${current.githubUser}/${current.githubRepo}` });
});

$("gh-sign-in").addEventListener("click", async () => {
  const error = $("github-error"), attempt = ++signInAttempt;
  error.textContent = "";
  let code;
  try {
    code = await invoke("github_sign_in_start");
  } catch (err) {
    error.textContent = String(err);
    return;
  }
  $("gh-code").textContent = code.userCode;
  $("gh-open").onclick = () => invoke("open_url", { url: code.verificationUri });
  $("gh-copy").onclick = () => invoke("copy_html", { html: code.userCode, text: code.userCode })
    .then(() => { $("gh-copy").textContent = "Copied"; });
  $("gh-copy").textContent = "Copy code";
  signingIn = true;
  showGithub();
  $("gh-copy").focus();
  try {
    await invoke("github_sign_in_wait"); // settings-changed then shows the signed-in view
  } catch (err) {
    if (attempt === signInAttempt && signingIn) error.textContent = String(err);
  }
  if (attempt !== signInAttempt) return; // cancelled and started again meanwhile
  signingIn = false;
  showGithub();
});
$("gh-cancel").addEventListener("click", () => {
  signingIn = false; // set first, so the wait's "cancelled" error isn't shown
  invoke("github_sign_in_cancel");
  showGithub();
  $("gh-sign-in").focus();
});
$("gh-sign-out").addEventListener("click", () => {
  invoke("github_sign_out").catch((err) => { $("github-error").textContent = String(err); });
});

listen("backup-changed", (e) => renderBackup(e.payload));
invoke("backup_status").then(renderBackup);

// Queued behind saves, so the focus after the folder picker can't show (and later re-save) the old folder.
const load = () => { saves = saves.then(() => invoke("get_settings").then(render)); };
load();
window.addEventListener("focus", load);
listen("settings-changed", (e) => render(e.payload));
