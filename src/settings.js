import { applyTheme } from "./theme.js";

const { invoke } = window.__TAURI__.core;
const { listen, emitTo } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);
const isMac = navigator.userAgent.includes("Mac");
let current = null, recording = null, saves = Promise.resolve(), statusTimer;
let backup = null, signingIn = false, signInAttempt = 0;
let cloudSigningIn = null, cloudAttempt = 0; // the provider whose browser sign-in is open

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
// The macOS menu bar is app-wide, and its Undo / Redo items act on the main window: undo typing here instead,
// and keep the keys from reaching that menu (a handled keydown stops its accelerator).
document.addEventListener("keydown", (e) => {
  if (!(e.metaKey || e.ctrlKey) || e.key.toLowerCase() !== "z") return;
  e.preventDefault();
  document.execCommand(e.shiftKey ? "redo" : "undo");
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

for (const button of document.querySelectorAll("[data-clear]")) {
  button.addEventListener("click", () => save({ [button.dataset.clear]: "" }));
}

// ---------- load, show and save ----------

function render(s) {
  if (!s) return;
  current = s;
  applyTheme(s.theme);
  for (const key of ["quickNote", "capture", "newSession", "newPage"]) {
    if (recording?.dataset.record !== key) $(`${key}-keys`).textContent = s[key] ? readable(s[key]) : "Not set";
  }
  for (const button of document.querySelectorAll("[data-clear]")) button.disabled = !s[button.dataset.clear];
  renderCampaigns(s);
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
  for (const p in CLOUDS) cloudEl(p, "cloud-user").textContent = s[`${p}User`];
  showClouds();
  renderBackup(backup);
  invoke("campaign_places").then(showPlaces).catch(() => {});
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

// ---------- campaigns: one row each, above the Add row ----------

const campaignName = (path) => path.split(/[\\/]/).filter(Boolean).pop() || path;

function renderCampaigns(s) {
  for (const row of document.querySelectorAll(".campaign")) row.remove();
  for (const path of s.campaigns) {
    const row = $("campaign-row").content.firstElementChild.cloneNode(true);
    const name = campaignName(path), active = path === s.vaultPath;
    row.querySelector(".campaign-name").textContent = active ? `${name} (open now)` : name;
    row.querySelector(".campaign-path").textContent = path;
    row.querySelector(".campaign-sr").textContent = ` ${name}`;
    const remove = row.querySelector(".campaign-remove");
    remove.disabled = active; // switch to another campaign first
    remove.addEventListener("click", () => save({ campaigns: current.campaigns.filter((c) => c !== path) }));
    $("campaign-add").before(row);
  }
}

/** Adds a campaign (unless it's there) and opens it: the main window saves the page it has open, then switches. */
function addCampaign(path) {
  const done = saves.then(async () => {
    if (!current.campaigns.includes(path)) render(await invoke("save_settings", { settings: { ...current, campaigns: [...current.campaigns, path] } }));
    await emitTo("main", "switch-campaign", path);
  });
  saves = done.catch(() => {}); // a failure here mustn't stop later saves
  return done;
}

$("choose").addEventListener("click", async () => {
  const path = await invoke("pick_folder", { title: "Choose the new campaign's notes folder", start: "" });
  if (!path) return;
  $("campaigns-error").textContent = "";
  addCampaign(path).catch((err) => { $("campaigns-error").textContent = String(err); });
});
$("reveal").addEventListener("click", () => invoke("open_vault_folder"));

// ---------- updates: the result shows in a native dialog ----------

window.__TAURI__.app?.getVersion().then((v) => { $("app-version").textContent = `Version ${v}`; }).catch(() => {});
$("check-updates").addEventListener("click", () => invoke("check_for_updates"));

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
  $("folder-restore").disabled = !current.backupFolder;
  $("gh-unavailable").hidden = status.githubAvailable;
  $("gh-sign-in").disabled = !status.githubAvailable;
  for (const p in CLOUDS) {
    const user = current[`${p}User`], t = status[p] ?? {};
    cloudEl(p, "cloud-status").textContent = targetStatus(t, user);
    cloudEl(p, "cloud-run-error").textContent = user ? t.lastError ?? "" : "";
    const again = !!user && /Sign in again\.$/.test(t.lastError ?? "");
    cloudEl(p, "cloud-again").hidden = !again;
    cloudEl(p, "cloud-now").hidden = again; // it can't work until then
    cloudEl(p, "cloud-now").disabled = status.running;
    cloudEl(p, "cloud-unavailable").hidden = status[`${p}Available`];
    cloudEl(p, "cloud-sign-in").disabled = !status[`${p}Available`];
  }
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
$("folder-restore").addEventListener("click", () => openRestore("folder", "your backup folder"));
$("gh-restore").addEventListener("click", () => openRestore("github", "GitHub"));
$("gh-repo").addEventListener("click", (e) => {
  e.preventDefault();
  invoke("open_url", { url: `https://github.com/${current.githubUser}/${places?.repo ?? current.githubRepo}` });
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

// ---------- Dropbox and Google Drive: sign-in happens in the browser ----------

const CLOUDS = {
  dropbox: { name: "Dropbox", account: "Dropbox", where: (w) => `${["Apps/Lorekeeper", w.dropbox].filter(Boolean).join("/")} in your Dropbox`,
    help: "Only new and changed notes are uploaded. A note you delete here is deleted there too, and Dropbox keeps deleted files for 30 days or more, so you can restore them." },
  google: { name: "Google Drive", account: "Google", where: (w) => `the ${w.drive} folder in your Google Drive`,
    help: "Only new and changed notes are uploaded. A note you delete here moves to the Google Drive trash, where you can restore it for 30 days." },
};
const cloudCards = {};
const cloudEl = (p, cls) => cloudCards[p].querySelector(`.${cls}`);

for (const [p, c] of Object.entries(CLOUDS)) {
  const card = document.createElement("div");
  card.append($("cloud-card").content.cloneNode(true));
  cloudCards[p] = card;
  $("clouds").append(card);
  card.querySelector("h2").textContent = `Back up to ${c.name}`;
  cloudEl(p, "cloud-label").textContent = `${c.account} account`;
  cloudEl(p, "cloud-sign-in").textContent = `Sign in with ${c.account}`;
  cloudEl(p, "cloud-unavailable").textContent = `${c.name} backup isn't set up in this build yet.`;
  for (const el of card.querySelectorAll(".cloud-sr")) el.textContent = ` ${c.name}`;
  cloudEl(p, "cloud-help").textContent = c.help;
  cloudEl(p, "cloud-sign-in").addEventListener("click", () => cloudSignIn(p));
  cloudEl(p, "cloud-again").addEventListener("click", () => cloudSignIn(p));
  cloudEl(p, "cloud-now").addEventListener("click", () => invoke("backup_now"));
  cloudEl(p, "cloud-restore").addEventListener("click", () => openRestore(p, c.name));
  cloudEl(p, "cloud-cancel").addEventListener("click", () => {
    cloudSigningIn = null; // set first, so the sign-in's "cancelled" error isn't shown
    invoke("cloud_sign_in_cancel");
    showClouds();
    cloudEl(p, current[`${p}User`] ? "cloud-again" : "cloud-sign-in").focus();
  });
  cloudEl(p, "cloud-sign-out").addEventListener("click", async () => {
    try {
      await invoke("cloud_sign_out", { provider: p });
      current = { ...current, [`${p}User`]: "" }; // settings-changed may come a moment later
      showClouds();
      cloudEl(p, "cloud-sign-in").focus();
    } catch (err) {
      cloudEl(p, "cloud-error").textContent = String(err);
    }
  });
}

// Where the open campaign backs up (restore.rs Places): another campaign than the first has places of its own.
let places = null;
function showPlaces(w) {
  places = w;
  for (const [p, c] of Object.entries(CLOUDS)) {
    cloudEl(p, "cloud-where").textContent = `Notes go to ${c.where(w)}. Lorekeeper can only see that folder.`;
    cloudEl(p, "cloud-dest").textContent = `Notes go to ${c.where(w)}.`;
  }
  if (current && w.repo) {
    $("gh-repo").textContent = `${current.githubUser}/${w.repo}`;
    $("backupFolder").textContent = w.folder || "Off";
  }
}
showPlaces({ dropbox: "", drive: "Lorekeeper" }); // the first campaign's, until campaign_places answers

function showClouds() {
  if (!current) return;
  for (const p in CLOUDS) {
    const user = current[`${p}User`], flow = cloudSigningIn === p;
    cloudEl(p, "cloud-out").hidden = flow || !!user;
    cloudEl(p, "cloud-flow").hidden = !flow;
    cloudEl(p, "cloud-in").hidden = flow || !user;
  }
}

// Starting another provider's sign-in cancels this one (one browser sign-in at a time).
async function cloudSignIn(p) {
  const attempt = ++cloudAttempt;
  for (const q in CLOUDS) cloudEl(q, "cloud-error").textContent = "";
  cloudSigningIn = p;
  showClouds();
  cloudEl(p, "cloud-cancel").focus();
  try {
    const account = await invoke("cloud_sign_in", { provider: p });
    current = { ...current, [`${p}User`]: account }; // settings-changed may come a moment later
    cloudEl(p, "cloud-user").textContent = account;
  } catch (err) {
    if (attempt === cloudAttempt && cloudSigningIn === p) cloudEl(p, "cloud-error").textContent = String(err);
  }
  if (attempt !== cloudAttempt) return; // cancelled, or another sign-in started meanwhile
  cloudSigningIn = null;
  showClouds();
  cloudEl(p, current[`${p}User`] ? "cloud-now" : "cloud-sign-in").focus();
}

// ---------- restore: lists a backup, downloads it into a new folder, never over the notes folder ----------

let restore = null; // the open dialog's state: { source, parent, target, done, running }

function restoreView() {
  const r = restore, busy = r.running;
  $("restore-pick").hidden = !!r.done;
  $("restore-actions").hidden = !!r.done;
  $("restore-after").hidden = !r.done;
  $("restore-target").textContent = r.target;
  $("restore-choice").disabled = $("restore-change").disabled = busy || !$("restore-choice").options.length;
  $("restore-go").disabled = busy || !r.target || !$("restore-choice").options.length;
  $("restore-cancel").disabled = busy;
  $("restore-progress").hidden = !busy;
}

async function openRestore(source, name) {
  const r = restore = { source, parent: "", target: "", done: null, running: false };
  $("restore-title").textContent = `Restore ${campaignName(current.vaultPath)} from ${name}`;
  $("restore-choice").replaceChildren();
  $("restore-status").textContent = "Looking for backups…";
  $("restore-error").textContent = "";
  restoreView();
  $("restore-dialog").showModal();
  const [list, target] = await Promise.allSettled([invoke("restore_list", { source }), invoke("restore_target", { parent: "" })]);
  if (restore !== r) return; // closed and opened again meanwhile
  $("restore-status").textContent = "";
  if (list.status === "fulfilled") {
    $("restore-choice").replaceChildren(...list.value.map((c) => new Option(c.label, c.id)));
  } else {
    $("restore-error").textContent = String(list.reason);
  }
  r.target = target.value ?? "";
  restoreView();
  if (list.status === "fulfilled") $("restore-choice").focus();
}

$("restore-change").addEventListener("click", async () => {
  const r = restore;
  const parent = await invoke("pick_folder", { title: "Choose where the restored notes go", start: "" });
  if (parent && restore === r) {
    r.parent = parent;
    r.target = await invoke("restore_target", { parent });
    restoreView();
  }
});

$("restore-go").addEventListener("click", async () => {
  const r = restore;
  r.running = true;
  $("restore-error").textContent = "";
  $("restore-status").textContent = "Restoring…";
  $("restore-progress").removeAttribute("value");
  restoreView();
  try {
    r.done = await invoke("restore_start", { source: r.source, id: $("restore-choice").value, target: r.target });
    const skipped = r.done.skipped.length ? ` Skipped ${r.done.skipped.length} that can't be saved here: ${r.done.skipped.join(", ")}.` : "";
    $("restore-status").textContent = `Restored ${r.done.files} ${r.done.files === 1 ? "file" : "files"} to ${r.done.target}.${skipped}`;
  } catch (err) {
    $("restore-status").textContent = "";
    $("restore-error").textContent = String(err);
    // A failed restore can leave a partly filled folder; the next try goes to a fresh one.
    r.target = await invoke("restore_target", { parent: r.parent }).catch(() => r.target);
  }
  r.running = false;
  restoreView();
  $(r.done ? "restore-close" : "restore-go").focus();
});

listen("restore-progress", ({ payload: [done, total] }) => {
  if (!restore?.running) return;
  $("restore-progress").max = total;
  $("restore-progress").value = done;
  $("restore-status").textContent = `Restoring ${Math.min(done + 1, total)} of ${total}…`;
});

$("restore-open").addEventListener("click", () => invoke("restore_open"));
$("restore-use").addEventListener("click", () => {
  // A new campaign with backups of its own: its first backup can't touch the backup it came from.
  const target = restore.done.target;
  addCampaign(target).then(() => {
    $("restore-status").textContent = `Opened ${campaignName(target)} as a new campaign. Your other campaigns are still where they were.`;
    $("restore-use").disabled = true;
  }, (err) => { $("restore-error").textContent = String(err); });
});
for (const id of ["restore-cancel", "restore-close"]) $(id).addEventListener("click", () => $("restore-dialog").close());
// Esc can't close it while a restore is running.
$("restore-dialog").addEventListener("cancel", (e) => { if (restore?.running) e.preventDefault(); });
$("restore-dialog").addEventListener("close", () => {
  $("restore-use").disabled = false;
  restore = null;
});

listen("backup-changed", (e) => renderBackup(e.payload));
invoke("backup_status").then(renderBackup);

// Queued behind saves, so the focus after the folder picker can't show (and later re-save) the old folder.
const load = () => { saves = saves.then(() => invoke("get_settings").then(render)); };
load();
window.addEventListener("focus", load);
listen("settings-changed", (e) => render(e.payload));
