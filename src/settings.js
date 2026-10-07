import { applyTheme } from "./theme.js";
import { joinText, onlineText, roleText, syncText } from "./sync-status.js";
import { DM, privateLabel } from "./vault.js";
import { confirmClick } from "./confirm.js";

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
// The webview's own menu (Reload, Back, Print, Inspect…) is for web pages: only text fields keep it, for Cut, Copy,
// Paste and spelling (as in app.js).
document.addEventListener("contextmenu", (e) => e.target.closest?.("textarea, input:not([type=checkbox], [type=radio])") || e.preventDefault());
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
  if (!current && s.syncServer) $("advanced").open = true; // once: a server you set yourself is worth seeing
  current = s;
  applyTheme(s.theme);
  for (const key of ["quickNote", "capture", "newSession", "newPage"]) {
    if (recording?.dataset.record !== key) $(`${key}-keys`).textContent = s[key] ? readable(s[key]) : "Not set";
  }
  for (const button of document.querySelectorAll("[data-clear]")) button.disabled = !s[button.dataset.clear];
  renderCampaigns(s);
  renderParty();
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

const folderName = (path) => path.split(/[\\/]/).filter(Boolean).pop() || path;
/** A campaign's name: the one given here, else its folder's name. */
const campaignName = (path) => current.backupNames?.[path] || folderName(path);

const pcs = new Map(); // campaign folder -> its PC pages (campaign_pcs)

/** Fills an "I play" list with "I'm the DM (no character)" and a campaign's PC pages, `me` chosen (kept even when its
 * page isn't there). */
function fillPcs(select, path, me) {
  const options = [...new Set([...(pcs.get(path) ?? []), ...(me && me !== DM ? [me] : [])])];
  select.replaceChildren(new Option("Choose…", ""), new Option("I'm the DM (no character)", DM),
    ...options.map((p) => new Option(p.replace(/^PCs\//, "").replace(/\.md$/i, ""), p)));
  select.value = me;
}

// ---------- shared campaigns: a status line and one button per row; the Party dialog does the rest (shared.rs) ----------
// Names and messages here can come from teammates or the server: they're only ever set as text.

const snapshots = new Map(); // campaign folder -> { status, online, warning } from sync-status events
/** You own the campaign, or the server says you're a DM who manages players. */
const manages = (sh) => sh.role === "owner" || (sh.access?.role === "dm" && !!sh.access?.manage);
let syncInfo = null, partyPath = null, sharingNow = false, askKey = false, linksTimer, privateBusy = false;
let unshare = null; // a campaign Share turned sharing on for: turned back off if Share doesn't finish (its notes would
// otherwise go to per-character files in a campaign nobody shares)

/** A campaign row's sharing line and its one button: Share with party… until it's shared, then Party…. */
function renderSharing(row, s, path) {
  const sh = s.sharing?.[path] ?? {};
  const open = row.querySelector(".campaign-party-open");
  open.hidden = !!sh.removed;
  open.firstChild.textContent = sh.room ? "Party…" : "Share with party…";
  open.addEventListener("click", () => openParty(path));
  showSyncStatus(row, path);
}

/** What a shared campaign is doing (every shared campaign syncs, open or not), and what to do next. */
function sharingText(path, row = true) {
  const sh = current.sharing?.[path] ?? {}, snap = snapshots.get(path);
  const pick = row && sh.shared && !sh.removed && !sh.me ? `Pick your character: click ${sh.room ? "Party…" : "Share with party…"}` : "";
  if (sh.shared && !sh.room && !sh.removed) return pick || "Not shared yet.";
  const online = sh.room ? onlineText(snap) : "";
  return [syncText(snap, sh, campaignName(path)), online, pick].filter(Boolean).join(" · ");
}

function showSyncStatus(row, path) {
  row.querySelector(".campaign-sync-status").textContent = sharingText(path);
}

const rowFor = (path) => [...document.querySelectorAll(".campaign")].find((r) => r.querySelector(".campaign-path").textContent === path);

listen("sync-status", ({ payload }) => {
  snapshots.set(payload.path, payload);
  const row = rowFor(payload.path);
  if (row && current) showSyncStatus(row, payload.path);
  if (partyPath === payload.path) renderParty();
  if (joinPath === payload.path) showJoin();
});

async function loadSyncInfo() {
  syncInfo = await invoke("sync_info").catch(() => null);
  if (!syncInfo) return;
  for (const snap of syncInfo.statuses) snapshots.set(snap.path, snap);
  const server = syncInfo.server;
  $("sync-server-in-use").textContent = server.Ok ? `Now using ${server.Ok}` : server.Err;
  if (server.Err) $("advanced").open = true;
  $("syncServer").placeholder = syncInfo.defaultServer;
  if (current) render(current);
}

const changeSharing = (next, path = partyPath) =>
  save({ sharing: { ...current.sharing, [path]: { ...(current.sharing?.[path] ?? { shared: false, me: "" }), ...next } } });

function refreshPcs(path) {
  invoke("campaign_pcs", { path }).then((list) => {
    pcs.set(path, list);
    if (partyPath !== path) return;
    fillPcs($("party-me"), path, $("party-me").value);
    renderParty();
  }).catch(() => {});
}

// The Party dialog, for one campaign. Not shared yet: pick who you play, then Share. Shared: your character, invites
// and players (the owner), and the switch that pauses syncing. Invite links hold the campaign's key, so they're made
// only on a click, copied only on Copy, and cleared when the dialog closes (or after 10 minutes).
function openParty(path, note = "") {
  partyPath = path;
  $("players-error").textContent = $("sharing-error").textContent = "";
  $("players-status").textContent = note;
  askKey = false;
  $("party-key-input").value = "";
  clearLinks();
  fillPcs($("party-me"), path, current.sharing?.[path]?.me ?? "");
  renderParty();
  refreshPcs(path);
  if (!$("players-dialog").open) $("players-dialog").showModal();
  const sh = current.sharing?.[path] ?? {};
  $(sh.room && sh.me ? "players-close" : "party-me").focus();
  $("invite-role").value = "player";
  $("invite-manage").checked = false;
  showInviteRole();
  if (sh.room && manages(sh) && !sh.removed) loadPlayers();
}

function renderParty() {
  const path = partyPath;
  if (!path || !current) return;
  const sh = current.sharing?.[path] ?? {}, setup = !sh.room, select = $("party-me");
  $("players-title").textContent = setup ? `Share ${campaignName(path)} with your party` : `Party: ${campaignName(path)}`;
  $("party-intro").hidden = !setup;
  $("party-key").hidden = !askKey;
  $("party-sync").textContent = setup ? "" : sharingText(path, false);
  if (document.activeElement !== select) fillPcs(select, path, setup ? select.value : sh.me);
  $("party-no-pcs").hidden = select.options.length > 2 || select.value === DM; // more than Choose… and the DM
  $("party-dm-hint").hidden = select.value !== DM;
  $("party-dm-hint").textContent = select.value === DM ? dmHint(sh) : ""; // it describes the list
  $("party-invites").hidden = !(sh.room && manages(sh) && !sh.removed);
  renderPrivate(sh);
  $("party-pause").hidden = setup && (!sh.shared || unshare === path); // also an old never-shared one, so it can be turned off
  $("party-shared").checked = !!sh.shared;
  const owner = sh.role === "owner";
  $("party-end").hidden = !sh.room || !!sh.removed;
  $("party-end-go").textContent = owner ? "Stop sharing" : "Leave";
  $("party-end-help").textContent = owner
    ? "Deletes the campaign from the sync server, everyone's private notes there included. Your players keep their copies but stop syncing. Your notes stay here."
    : "Leaves the party: this computer stops syncing, and your private notes are deleted from the sync server. Your copy stays here.";
  $("party-cancel").hidden = $("party-share").hidden = !setup;
  $("party-cancel").disabled = $("party-shared").disabled = sharingNow; // closing mid-Share would switch sharing off under it
  $("players-close").hidden = setup;
  $("party-share").disabled = sharingNow || !select.value;
  select.disabled = sharingNow;
}

/**
 * Under I play: "I'm the DM (no character)" only files your notes as the DM's. Reading private notes is separate (the
 * owner's I'm the DM switch, or a DM's role), so the owner is pointed at the switch.
 */
function dmHint(sh) {
  const notes = "Your notes show as the DM's, with no portrait.";
  const owner = sh.role === "owner" || (!sh.room && !sh.removed); // sharing it makes you the owner
  if (!owner) return `${notes} Whether you read your players' private notes is up to the campaign's owner.`;
  if (sh.access?.ownerIsDm) return `${notes} You also read your players' private notes because I'm the DM is on under Private notes, a separate setting.`;
  return `${notes} To read your players' private notes too, also turn on I'm the DM under Private notes${sh.room ? "" : " once the campaign is shared"}. It's a separate setting.`;
}

/**
 * Who reads private notes, as the sync server last said (sh.access), so the line is always the truth. The owner sets it
 * here, and says whether they're also the DM; everyone else sees their role and the setting.
 */
function renderPrivate(sh) {
  const owner = sh.role === "owner";
  $("party-private").hidden = !sh.room || !!sh.removed;
  if ($("party-private").hidden) return;
  const role = owner ? (sh.access?.ownerIsDm ? "the owner and the DM" : "the owner") : sh.access?.role ? `a ${roleText(sh.access.role, sh.access.manage)}` : "";
  $("party-role").textContent = role ? `You're ${role}. Your private notes: ${privateLabel(sh).replace(/^Private: /, "")}.` : `${privateLabel(sh)}.`;
  $("party-private-owner").hidden = !owner;
  $("party-private-select").value = sh.access?.dmReadsPrivate ? "yes" : "no";
  $("party-owner-dm").checked = !!sh.access?.ownerIsDm;
  $("party-private-select").disabled = $("party-owner-dm").disabled = privateBusy;
}

/** Runs an owner's private-notes change (sync_set_dm_reads, sync_set_owner_dm), then shows what the server now says. */
async function changePrivate(command, args) {
  const path = partyPath;
  privateBusy = true;
  renderParty();
  $("players-error").textContent = "";
  try {
    await invoke(command, { path, ...args });
  } catch (err) {
    $("players-error").textContent = String(err);
  }
  privateBusy = false;
  render(await invoke("get_settings")); // redraws the dialog too
}
$("party-private-select").addEventListener("change", () => changePrivate("sync_set_dm_reads", { on: $("party-private-select").value === "yes" }));
$("party-owner-dm").addEventListener("change", () => changePrivate("sync_set_owner_dm", { on: $("party-owner-dm").checked }));

/** Invite as a DM: only the owner can let them manage players. The button says who the link is for. */
function showInviteRole() {
  const dm = $("invite-role").value === "dm";
  $("invite-manage-row").hidden = !dm || current?.sharing?.[partyPath]?.role !== "owner";
  $("invite-one").textContent = dm ? "Invite a DM" : "Invite a player";
}
$("invite-role").addEventListener("change", showInviteRole);

$("party-me").addEventListener("change", () => {
  if (current.sharing?.[partyPath]?.room) changeSharing({ me: $("party-me").value });
  else renderParty();
});
$("party-shared").addEventListener("change", () => changeSharing({ shared: $("party-shared").checked }));

/** Share: turns sharing on with your character, then makes the campaign's room. Asks for the server's creation key
 * when it wants one (the field stays open until it works). */
async function share() {
  const path = partyPath, me = $("party-me").value;
  if (sharingNow || !me) return;
  if (!current.sharing?.[path]?.shared) unshare = path;
  sharingNow = true;
  $("players-error").textContent = "";
  $("players-status").textContent = "Sharing…";
  renderParty();
  changeSharing({ shared: true, me }, path);
  await saves;
  let shared = false;
  if (current.sharing?.[path]?.shared) { // else the save's error shows
    try {
      await invoke("sync_share", { path, createKey: askKey ? $("party-key-input").value : null });
      shared = true;
    } catch (err) {
      if (String(err) === "create_key_required") {
        askKey = true;
        $("players-status").textContent = "This server needs its creation key.";
      } else {
        $("players-error").textContent = String(err);
      }
    }
  }
  if (shared) unshare = null;
  else if (!askKey) undoShare();
  if (!shared && $("players-status").textContent === "Sharing…") $("players-status").textContent = "";
  sharingNow = false;
  if (partyPath !== path) return;
  if (shared) {
    askKey = false;
    $("players-status").textContent = "Shared. Now invite your players: each one gets their own link.";
    load(); // the room Share made: shows the invites
    await saves;
    $("invite-one").focus();
    loadPlayers();
  } else {
    renderParty();
    $(askKey ? "party-key-input" : "party-share").focus();
  }
}
$("party-share").addEventListener("click", share);
$("party-key-input").addEventListener("keydown", (e) => { if (e.key === "Enter") share(); });
$("party-cancel").addEventListener("click", () => $("players-dialog").close());

function clearLinks() {
  clearTimeout(linksTimer);
  $("invite-text").value = "";
  $("invite-links").hidden = true;
}

/** Shows a new link (an invite or a re-invite) once: Copy copies it; cleared on close or after 10 minutes. */
function showLink(link, hint) {
  $("invite-text").value = link;
  $("invite-hint").textContent = hint;
  $("invite-links").hidden = false;
  clearTimeout(linksTimer);
  linksTimer = setTimeout(clearLinks, 10 * 60 * 1000);
  $("invite-copy").focus();
}

async function makeInvites() {
  const path = partyPath;
  $("players-error").textContent = "";
  const role = $("invite-role").value;
  const manage = role === "dm" && !$("invite-manage-row").hidden && $("invite-manage").checked;
  try {
    const links = await invoke("sync_invite", { path, count: 1, role, manage });
    if (partyPath !== path) return;
    const who = role === "dm" ? (manage ? "a DM who manages players" : "a DM") : "a player";
    showLink(links.join("\n"), `The link lets one person join as ${who}, once, within 7 days. Send it to them privately: it holds the campaign's key. Make a new one for each.`);
    await loadPlayers();
  } catch (err) {
    $("players-error").textContent = String(err);
  }
}

/** Player, DM, or a DM who manages players, in the owner's role picker: role and manage as one value. */
const ROLE_CHOICES = [["player", "Player"], ["dm", "DM"], ["dm-manage", "DM who manages players"]];

const lastSeen = (ms) => (ms ? `last seen ${when(new Date(ms).toISOString())}` : "hasn't connected yet");

async function loadPlayers() {
  const path = partyPath;
  const [invites, players] = await Promise.allSettled([invoke("sync_invites", { path }), invoke("sync_members", { path })]);
  if (partyPath !== path) return;
  const failed = [invites, players].find((r) => r.status === "rejected");
  if (failed) $("players-error").textContent = String(failed.reason);
  const item = (text, button, action) => {
    const li = document.createElement("li");
    const span = document.createElement("span");
    span.textContent = text;
    li.append(span);
    if (button) {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "ghost";
      b.textContent = button;
      b.addEventListener("click", () => action(b));
      li.append(b);
    }
    return li;
  };
  const pending = invites.value ?? [];
  const kind = (i) => (i.reinvite ? "Re-invite" : `Invite as ${roleText(i.role, i.manage)}`);
  $("invite-list").replaceChildren(...(pending.length ? pending.map((i) => item(`${kind(i)}, expires ${when(new Date(i.expires).toISOString())}`, "Cancel", async () => {
    await invoke("sync_cancel_invite", { path, invite: i.invite }).catch((err) => { $("players-error").textContent = String(err); });
    loadPlayers();
  })) : [item("None")]));
  const online = new Set(snapshots.get(path)?.online ?? []);
  const owner = current.sharing?.[path]?.role === "owner";
  const fail = (err) => { $("players-error").textContent = String(err); };
  $("player-list").replaceChildren(...(players.value ?? []).map((p) => {
    const name = p.name || "No character picked yet";
    if (p.owner) return item(`${name} (${p.you ? "you, " : ""}the owner${p.ownerIsDm ? " and the DM" : ""})`);
    const seen = p.you ? "you" : online.has(p.name) ? "online now" : lastSeen(p.lastSeen);
    const li = item(owner ? `${name}, ${seen}` : `${name}, ${roleText(p.role, p.manage)}, ${seen}`);
    // The owner changes roles; a manager handles only members who don't manage (the server checks again).
    if (owner) {
      const select = document.createElement("select");
      select.setAttribute("aria-label", `Role of ${name}`);
      for (const [value, label] of ROLE_CHOICES) select.append(new Option(label, value));
      select.value = p.role === "dm" ? (p.manage ? "dm-manage" : "dm") : "player";
      select.addEventListener("change", async () => {
        const [role, manage] = select.value === "dm-manage" ? ["dm", true] : [select.value, false];
        await invoke("sync_set_role", { path, memberId: p.memberId, role, manage }).catch(fail);
        loadPlayers();
      });
      li.append(select);
    }
    if (p.you || (!owner && p.manage)) return li;
    const button = (text, action) => {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "ghost";
      b.textContent = text;
      if (action) b.addEventListener("click", () => action(b));
      li.append(b);
      return b;
    };
    button("Re-invite", async () => {
      try {
        const link = await invoke("sync_reinvite", { path, memberId: p.memberId });
        if (partyPath !== path) return;
        showLink(link, `Re-invite for ${name}, for their new computer: whoever opens it becomes ${name}, with their role and private notes, once, within 7 days. ${name}'s old computer stops syncing and says Signed in on another computer. Send it only to ${name}, privately.`);
        loadPlayers();
      } catch (err) {
        fail(err);
      }
    });
    // Two clicks: the first asks, out loud too (the status line is live).
    confirmClick(button("Remove"), {
      armedLabel: `Remove ${name}? Click again`,
      prompt: `Click again to remove ${name}.`,
      say: (text) => { if (text || $("players-status").textContent === `Click again to remove ${name}.`) $("players-status").textContent = text; }, // clears only its own prompt
      act: async () => {
        await invoke("sync_remove_member", { path, memberId: p.memberId }).catch(fail);
        loadPlayers();
      },
    });
    return li;
  }));
}

$("invite-one").addEventListener("click", makeInvites);
$("invite-copy").addEventListener("click", () => navigator.clipboard.writeText($("invite-text").value)
  .then(() => { $("players-status").textContent = "Copied. Send each player their own link."; },
    (err) => { $("players-error").textContent = `Copy failed: ${err}`; }));
$("players-close").addEventListener("click", () => $("players-dialog").close());
$("players-dialog").addEventListener("cancel", (e) => { if (sharingNow) e.preventDefault(); });
function undoShare() {
  if (unshare && !current.sharing?.[unshare]?.room) changeSharing({ shared: false }, unshare);
  unshare = null;
}
$("players-dialog").addEventListener("close", () => {
  undoShare();
  clearLinks();
  partyPath = null;
});

// ---------- asking before Stop sharing, Leave and Delete campaign ----------

const trashName = isMac ? "Trash" : navigator.userAgent.includes("Windows") ? "Recycle Bin" : "Trash";
for (const el of document.querySelectorAll(".trash-name")) el.textContent = trashName;

/**
 * Asks before something this window can't undo. `go` (and `alt`, a second choice) label the buttons; the chosen one's
 * `run(choice)` runs with the dialog open, so it can't be clicked twice and an error shows in the dialog. Resolves true
 * once a run succeeded, false when cancelled. Names in the text are set as text only.
 */
function confirmAction({ title, text, extra = "", go, alt = "", run }) {
  const dialog = $("confirm-dialog"), buttons = ["confirm-cancel", "confirm-alt", "confirm-go"].map($);
  $("confirm-title").textContent = title;
  $("confirm-text").textContent = text;
  $("confirm-extra").textContent = extra;
  $("confirm-error").textContent = $("confirm-status").textContent = "";
  $("confirm-go").textContent = go;
  $("confirm-alt").textContent = alt;
  $("confirm-alt").hidden = !alt;
  let busy = false;
  return new Promise((resolve) => {
    const choose = async (choice) => {
      if (busy) return;
      busy = true;
      for (const b of buttons) b.disabled = true;
      $("confirm-error").textContent = "";
      $("confirm-status").textContent = "Working…";
      try {
        await run(choice);
        busy = false;
        resolve(true);
        dialog.close();
      } catch (err) {
        $("confirm-error").textContent = String(err);
      }
      busy = false;
      $("confirm-status").textContent = "";
      for (const b of buttons) b.disabled = false;
    };
    $("confirm-go").onclick = () => choose("go");
    $("confirm-alt").onclick = () => choose("alt");
    $("confirm-cancel").onclick = () => dialog.close();
    dialog.oncancel = (e) => { if (busy) e.preventDefault(); };
    dialog.onclose = () => resolve(false);
    dialog.showModal();
    $("confirm-cancel").focus(); // the safe choice first
  });
}

/** The Party dialog's last button: the owner stops sharing, anyone else leaves. */
$("party-end-go").addEventListener("click", async () => {
  const path = partyPath, name = campaignName(path);
  const owner = current.sharing?.[path]?.role === "owner";
  const done = await confirmAction(owner ? {
    title: `Stop sharing ${name}?`,
    text: `Your players keep their copies of ${name}, but lose access: their copies stop syncing. ${name} is deleted from the sync server, and so are everyone's private notes there.`,
    extra: "Your notes on this computer stay. You can share it again later; everyone then needs a new invite.",
    go: "Stop sharing",
    run: () => invoke("sync_stop_sharing", { path }),
  } : {
    title: `Leave ${name}?`,
    text: `This computer stops syncing ${name}, and you leave its party. Your private notes are deleted from the sync server.`,
    extra: "Your copy stays on this computer. To come back, ask the owner for a new invite.",
    go: "Leave",
    run: () => invoke("sync_leave", { path }),
  });
  if (!done) return;
  if ($("players-dialog").open) $("players-dialog").close();
  render(await invoke("get_settings"));
});

/** Delete campaign: to the Trash, after asking. The owner of a shared campaign is offered Stop sharing first. */
async function deleteCampaign(path) {
  const sh = current.sharing?.[path] ?? {}, name = campaignName(path);
  const owned = sh.role === "owner" && !!sh.room;
  let stopped = false;
  const done = await confirmAction({
    title: `Delete ${name}?`,
    text: `This moves the folder ${path} to the ${trashName}, with every note in it, and takes ${name} off this list. You can put the folder back from the ${trashName}.`,
    extra: owned ? `You share ${name}. Stop sharing it first, so it's deleted from the sync server too: your players keep their copies but lose access. Delete only leaves it on the server, and you can't manage it from Lorekeeper any more.` : "",
    go: owned ? "Stop sharing and delete" : "Delete",
    alt: owned ? "Delete only" : "",
    run: async (choice) => {
      if (owned && choice === "go" && !stopped) {
        await invoke("sync_stop_sharing", { path });
        stopped = true; // a failed delete after this doesn't stop sharing twice
      }
      await invoke("delete_campaign", { path });
    },
  });
  if (done) render(await invoke("get_settings"));
}

// Join: paste a link, see which server it's for, then join. The dialog then shows how it's going: waiting for the owner's
// notes, the campaign's name, the download, Done. The link holds the key: cleared once joined, or when the dialog closes.
let joinChecked = null, joining = false;
let joinProgress = null, joinPath = null; // the "sync-join" event's { waiting, name }, then the new campaign's folder

function resetJoin() {
  joinChecked = joinProgress = joinPath = null;
  $("join-confirm").hidden = $("join-name-row").hidden = $("join-progress").hidden = true;
  $("join-link-row").hidden = $("join-go").hidden = false;
  $("join-go").textContent = "Continue";
  $("join-cancel").textContent = "Cancel";
  $("join-name").value = "";
  $("join-error").textContent = $("join-status").textContent = "";
}

/** Join's progress line and bar (names come from the network: text only). */
function showJoin() {
  if (!joining && !joinPath) return;
  const snap = joinPath ? snapshots.get(joinPath) : null, s = snap?.status;
  const failed = ["stopped", "removed", "replaced"].includes(s?.state);
  const { text, done } = joinText(joinProgress, snap);
  $("join-status").textContent = failed ? "" : text;
  if (failed) $("join-error").textContent = text;
  const bar = $("join-progress");
  bar.hidden = done || failed;
  if (s?.state === "downloading" && s.total) {
    bar.max = s.total;
    bar.value = Math.min(s.done, s.total);
  } else {
    bar.removeAttribute("value"); // still going, how far unknown
  }
  if (done && $("join-go").hidden) {
    $("join-go").textContent = "Pick your character…";
    $("join-go").hidden = false;
    $("join-go").focus();
  }
}
listen("sync-join", ({ payload }) => {
  joinProgress = payload;
  showJoin();
});

$("join-open").addEventListener("click", () => {
  $("join-link").value = "";
  resetJoin();
  $("join-dialog").showModal();
  $("join-link").focus();
});
$("join-link").addEventListener("input", resetJoin);
$("join-cancel").addEventListener("click", () => $("join-dialog").close());
$("join-dialog").addEventListener("cancel", (e) => { if (joining) e.preventDefault(); });
$("join-dialog").addEventListener("close", () => {
  $("join-link").value = "";
  resetJoin();
});
/** Join's first step: reads the link without using it and shows which server it's for (warning when it isn't the
 * usual one). The second click joins. */
async function checkJoin() {
  const link = $("join-link").value.trim();
  try {
    const check = await invoke("sync_check_invite", { link });
    if ($("join-link").value.trim() !== link) return; // changed meanwhile
    joinChecked = link;
    $("join-server").textContent = `This invite is for the sync server at ${check.server}.`;
    $("join-warning").textContent = check.isDefault ? "" :
      `That isn't Lorekeeper's usual server (${syncInfo?.defaultServer ?? "lorekeeper.yonatankarp.com"}). Join only if you know and trust whoever runs it.`;
    $("join-confirm").hidden = false;
    $("join-go").textContent = "Join";
    $("join-go").focus();
  } catch (err) {
    $("join-error").textContent = String(err);
  }
}

// An invite opened from the join page (lorekeeper://, deeplink.rs): the link fills in the dialog and is checked, and
// joining still takes a click on Join. The link isn't in the event; this window takes it, once.
async function takeJoinLink() {
  const pending = await invoke("take_join_link").catch(() => null);
  if (!pending || joining) return;
  selectTab($("tab-general"));
  $("join-link").value = pending.link ?? "";
  resetJoin();
  if (!$("join-dialog").open) $("join-dialog").showModal();
  if (pending.error) {
    $("join-error").textContent = pending.error;
    $("join-link").focus();
    return;
  }
  await checkJoin();
}
listen("join-link", takeJoinLink);
takeJoinLink(); // started by a link: it may have come before this window was listening

$("join-go").addEventListener("click", async () => {
  if (joinPath) { // done: on to picking your character
    const path = joinPath;
    $("join-dialog").close();
    openParty(path, "Pick the character you play.");
    return;
  }
  $("join-error").textContent = "";
  if (!joinChecked) return checkJoin();
  joining = true;
  joinProgress = null;
  $("join-go").disabled = $("join-cancel").disabled = $("join-link").disabled = $("join-name").disabled = true;
  showJoin();
  try {
    const path = await invoke("sync_join", { link: joinChecked, name: $("join-name").value.trim() || null });
    joinPath = path;
    joinChecked = null;
    $("join-link").value = ""; // it holds the campaign's key
    $("join-link-row").hidden = $("join-confirm").hidden = $("join-name-row").hidden = $("join-go").hidden = true;
    $("join-cancel").textContent = "Close"; // the download goes on; Settings and the sidebar show it too
    joining = false;
    showJoin();
    render(await invoke("get_settings")); // the joined campaign, before anything saves on top of older settings
    await addCampaign(path); // opens it
  } catch (err) {
    if (String(err) === "name_required") {
      $("join-status").textContent = "";
      $("join-name-row").hidden = false;
      $("join-progress").hidden = true;
      $("join-name").disabled = false;
      $("join-name").focus();
    } else {
      $("join-status").textContent = "";
      $("join-progress").hidden = true;
      $("join-error").textContent = String(err);
    }
  }
  joining = false;
  $("join-go").disabled = $("join-cancel").disabled = $("join-link").disabled = $("join-name").disabled = false;
});

// PC pages arrive while a joined campaign downloads: refresh the I play list.
listen("vault-changed", () => { if (partyPath) refreshPcs(partyPath); });
loadSyncInfo();

function renderCampaigns(s) {
  // Redrawn on every settings change: a name being typed keeps its text and focus, a switch or list just used its focus.
  const typing = document.activeElement?.classList.contains("campaign-backup-name") ? document.activeElement : null;
  const used = ["campaign-party-open", "campaign-more-toggle"].find((c) => document.activeElement?.classList.contains(c));
  const usedPath = used && document.activeElement.closest(".campaign")?.querySelector(".campaign-path").textContent;
  const opened = new Set([...document.querySelectorAll(".campaign-more[open] .campaign-path")].map((p) => p.textContent));
  for (const row of document.querySelectorAll(".campaign")) row.remove();
  for (const path of s.campaigns) {
    const row = $("campaign-row").content.firstElementChild.cloneNode(true);
    const name = s.backupNames?.[path] || folderName(path), active = path === s.vaultPath;
    row.querySelector(".campaign-name").textContent = active ? `${name} (open now)` : name;
    row.querySelector(".campaign-path").textContent = path;
    for (const el of row.querySelectorAll(".campaign-sr")) el.textContent = ` ${name}`;
    // Saved on change, not per keystroke: each save starts a backup in the new name's places.
    const backupName = row.querySelector(".campaign-backup-name");
    backupName.value = typing?.dataset.path === path ? typing.value : s.backupNames?.[path] ?? "";
    backupName.dataset.path = path;
    backupName.placeholder = folderName(path);
    backupName.addEventListener("change", () => save({ backupNames: { ...current.backupNames, [path]: backupName.value.trim() } }));
    // The open campaign can't be removed or deleted: it says how, in place of the buttons.
    const remove = row.querySelector(".campaign-remove");
    remove.hidden = row.querySelector(".campaign-remove-hint").hidden = active;
    row.querySelector(".campaign-remove-why").hidden = !active;
    row.querySelector(".campaign-more").open = opened.has(path);
    remove.addEventListener("click", () => save({ campaigns: current.campaigns.filter((c) => c !== path) }));
    const del = row.querySelector(".campaign-delete");
    del.hidden = row.querySelector(".campaign-delete-hint").hidden = active;
    del.addEventListener("click", () => deleteCampaign(path));
    renderSharing(row, s, path);
    $("campaign-add").before(row);
    if (typing?.dataset.path === path) backupName.focus();
    if (usedPath === path) row.querySelector(`.${used}`).focus();
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

// Add campaign…: New campaign makes <Lorekeeper folder>/<name>/; Choose existing folder… adds a folder you have.
$("choose").addEventListener("click", () => {
  $("add-name").value = $("add-error").textContent = $("campaigns-error").textContent = "";
  $("add-dialog").showModal();
  $("add-name").focus();
});
$("add-cancel").addEventListener("click", () => $("add-dialog").close());
$("add-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  $("add-error").textContent = "";
  try {
    const path = await invoke("create_campaign", { name: $("add-name").value });
    $("add-dialog").close();
    render(await invoke("get_settings")); // with the new campaign, before anything saves on top of older settings
    await addCampaign(path); // opens it
  } catch (err) {
    $("add-error").textContent = String(err);
  }
});
$("add-existing").addEventListener("click", async () => {
  const path = await invoke("pick_folder", { title: "Choose the new campaign's notes folder", start: "" });
  if (!path) return;
  $("add-dialog").close();
  addCampaign(path).catch((err) => { $("campaigns-error").textContent = String(err); });
});

// The main window's first-run view: "Join a shared campaign…".
listen("open-join", () => {
  selectTab($("tab-general"));
  if (!$("join-dialog").open) $("join-open").click();
});

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
  $("folder-restore").disabled = $("folder-open").disabled = !current.backupFolder;
  $("gh-unavailable").hidden = status.githubAvailable;
  $("gh-sign-in").disabled = !status.githubAvailable;
  for (const p in CLOUDS) {
    const user = current[`${p}User`], t = status[p] ?? {};
    cloudEl(p, "cloud-status").textContent = targetStatus(t, user);
    cloudEl(p, "cloud-run-error").textContent = user ? t.lastError ?? "" : "";
    cloudEl(p, "cloud-again").hidden = !user || !/Sign in again\.$/.test(t.lastError ?? "");
    cloudEl(p, "cloud-unavailable").hidden = status[`${p}Available`];
    cloudEl(p, "cloud-sign-in").disabled = !status[`${p}Available`];
  }
  for (const t of ["dropbox", "google", "github"]) {
    $("backup-target").querySelector(`[value="${t}"]`).disabled = !status[`${t}Available`] && !targetOn(t);
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
$("folder-restore").addEventListener("click", () => openRestore("folder", "your backup folder"));
$("gh-restore").addEventListener("click", () => openRestore("github", "GitHub"));
// The open campaign's backup: the folder in the file manager, the others in the browser.
const openBackup = (source, error) => invoke("open_backup", { source }).catch((err) => { $(error).textContent = String(err); });
$("folder-open").addEventListener("click", () => openBackup("folder", "folder-run-error"));
for (const id of ["gh-open-repo", "gh-repo"]) {
  $(id).addEventListener("click", (e) => {
    e.preventDefault();
    openBackup("github", "github-error");
  });
}


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
  $("gh-copy").onclick = () => navigator.clipboard.writeText(code.userCode)
    .then(() => { $("gh-copy").textContent = "Copied"; }, (err) => { error.textContent = `Copy failed: ${err}`; });
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
    help: "Only changes are uploaded. A note you delete is deleted there too, but Dropbox keeps deleted files for at least 30 days." },
  google: { name: "Google Drive", account: "Google", where: (w) => `the ${w.drive} folder in your Google Drive`,
    help: "Only changes are uploaded. A note you delete goes to the Google Drive trash, where you can get it back for 30 days." },
};
const cloudCards = {};
const cloudEl = (p, cls) => cloudCards[p].querySelector(`.${cls}`);

for (const [p, c] of Object.entries(CLOUDS)) {
  const card = document.createElement("div");
  card.append($("cloud-card").content.cloneNode(true));
  cloudCards[p] = card;
  $("clouds").append(card);
  card.className = "backup-target";
  card.dataset.target = p;
  cloudEl(p, "cloud-label").textContent = `${c.account} account`;
  cloudEl(p, "cloud-sign-in").textContent = `Sign in with ${c.account}`;
  cloudEl(p, "cloud-unavailable").textContent = `${c.name} backup isn't set up in this build yet.`;
  for (const el of card.querySelectorAll(".cloud-sr")) el.textContent = ` ${c.name}`;
  cloudEl(p, "cloud-help").textContent = c.help;
  cloudEl(p, "cloud-sign-in").addEventListener("click", () => cloudSignIn(p));
  cloudEl(p, "cloud-again").addEventListener("click", () => cloudSignIn(p));
  cloudEl(p, "cloud-restore").addEventListener("click", () => openRestore(p, c.name));
  cloudEl(p, "cloud-open").textContent = `Open in ${c.name}`;
  cloudEl(p, "cloud-open").addEventListener("click", () => invoke("open_backup", { source: p }).catch((err) => { cloudEl(p, "cloud-error").textContent = String(err); }));
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

// Where the open campaign backs up (restore.rs Places): a folder named after it in each backup.
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
showPlaces({ dropbox: "", drive: "Lorekeeper" }); // the shared folders, until campaign_places answers

function showClouds() {
  if (!current) return;
  for (const p in CLOUDS) {
    const user = current[`${p}User`], flow = cloudSigningIn === p;
    cloudEl(p, "cloud-out").hidden = flow || !!user;
    cloudEl(p, "cloud-flow").hidden = !flow;
    cloudEl(p, "cloud-in").hidden = flow || !user;
  }
  showTarget();
}

// ---------- one backup target: the choice shows only its controls ----------
// Backups that were already on stay on until you turn them off; finishing a new target's setup here turns the others
// off (it says so first). Signing out keeps everything already backed up there.

const TARGETS = { folder: "your folder", dropbox: "Dropbox", google: "Google Drive", github: "GitHub" };
const targetOn = (t) => !!(t === "folder" ? current.backupFolder : current[`${t}User`]);
const names = (list) => new Intl.ListFormat("en").format(list.map((t) => TARGETS[t]));
let chosen = null, chosenOn = false, turningOff = false, offNote = "";

function showTarget() {
  if (!current) return;
  const on = Object.keys(TARGETS).filter(targetOn);
  if (chosen === null) { // on opening: the first backup that's on, and nothing turns off by itself
    chosen = on[0] ?? "off";
    chosenOn = chosen !== "off";
  }
  const isOn = chosen !== "off" && targetOn(chosen), others = on.filter((t) => t !== chosen);
  if (isOn && !chosenOn && others.length) turnOff(others, true); // its setup just finished
  chosenOn = isOn;
  $("backup-target").value = chosen;
  for (const el of document.querySelectorAll(".backup-target")) el.hidden = el.dataset.target !== chosen;
  const waiting = chosen !== "off" && !isOn;
  $("backup-others-text").textContent = !others.length ? offNote
    : waiting ? `Backups to ${names(others)} keep running until you finish setting this up. Then they stop.`
    : chosen === "off" ? `Backups to ${names(others)} are still on.`
    : `Backups to ${names(others)} are on too.`;
  $("backup-others").hidden = !$("backup-others-text").textContent;
  const actions = $("backup-others-actions"), key = waiting ? "" : others.join();
  if (actions.dataset.list !== key) {
    actions.dataset.list = key;
    actions.replaceChildren(...(waiting ? [] : others).map((t) => {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "ghost";
      b.textContent = `Turn off ${t === "folder" ? "folder" : TARGETS[t]}`;
      b.addEventListener("click", () => turnOff([t], false));
      return b;
    }));
  }
  for (const b of actions.children) b.disabled = turningOff;
}

/** Turns backups off one at a time: the sign-outs each rewrite the settings file. */
async function turnOff(list, auto) {
  if (turningOff) return;
  turningOff = true;
  $("backup-others-error").textContent = "";
  let failed = false;
  for (const t of list) {
    try {
      if (t === "folder") {
        save({ backupFolder: "" });
        await saves;
        if (current.backupFolder) throw $("backupFolder-error").textContent || "Couldn't turn off the folder backup."; // save shows, never throws
      } else {
        await invoke(t === "github" ? "github_sign_out" : "cloud_sign_out", t === "github" ? {} : { provider: t });
        current = { ...current, [`${t}User`]: "" }; // settings-changed may come a moment later
      }
    } catch (err) {
      $("backup-others-error").textContent = String(err);
      failed = true;
      break;
    }
  }
  turningOff = false;
  if (auto && !failed) offNote = `Backups to ${names(list)} are off now. The copies already there are kept.`;
  render(current);
}

$("backup-target").addEventListener("change", () => {
  if (signingIn) $("gh-cancel").click(); // a sign-in left open would finish out of sight
  if (cloudSigningIn) cloudEl(cloudSigningIn, "cloud-cancel").click();
  chosen = $("backup-target").value;
  chosenOn = chosen !== "off" && targetOn(chosen);
  offNote = "";
  showTarget();
  $("backup-target").focus();
});

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
  cloudEl(p, current[`${p}User`] ? "cloud-restore" : "cloud-sign-in").focus();
}

// ---------- D&D Beyond: sign-in happens in D&D Beyond's own page, in a window of its own ----------

let ddbSigningIn = false;
async function showDdb() {
  const on = await invoke("dndbeyond_status").catch(() => false);
  if (ddbSigningIn) return;
  $("ddb-status").textContent = on ? "Signed in" : "Not signed in";
  $("ddb-sign-in").hidden = on;
  $("ddb-sign-out").hidden = !on;
}
// Signed in but D&D Beyond refused the session: say why instead of waiting silently.
listen("dndbeyond-waiting", (e) => { if (ddbSigningIn) $("ddb-status").textContent = `Still waiting: ${e.payload}`; });
$("ddb-sign-in").addEventListener("click", async () => {
  ddbSigningIn = true;
  $("ddb-error").textContent = "";
  $("ddb-sign-in").disabled = true;
  $("ddb-status").textContent = "Finish signing in in the D&D Beyond window…";
  try {
    await invoke("dndbeyond_sign_in");
  } catch (err) {
    if (!/cancelled/.test(err)) $("ddb-error").textContent = String(err);
  }
  ddbSigningIn = false;
  $("ddb-sign-in").disabled = false;
  await showDdb();
  $($("ddb-sign-in").hidden ? "ddb-sign-out" : "ddb-sign-in").focus();
});
$("ddb-sign-out").addEventListener("click", async () => {
  $("ddb-error").textContent = "";
  try {
    await invoke("dndbeyond_sign_out");
  } catch (err) {
    $("ddb-error").textContent = String(err);
  }
  await showDdb();
  $("ddb-sign-in").focus();
});
listen("dndbeyond-changed", showDdb);
showDdb();

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
    $("restore-status").textContent = `Added ${campaignName(target)} as a new campaign; the notes window is opening it. Your other campaigns are still where they were.`;
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
listen("settings-changed", (e) => { render(e.payload); loadSyncInfo(); });
