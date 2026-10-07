// What a shared campaign's sync status says, in Settings and in the main window (shared.rs sends the snapshots).
// Everything here may come from teammates (names) or the server: callers set it as text, never as HTML.

/** The status line for a campaign's snapshot { status, online, warning }, its sharing settings and its name. */
export function syncText(snap, sharing, name = "this campaign") {
  if (sharing?.unshared) return `The owner stopped sharing ${name}. Your copy stays on this computer.`;
  if (sharing?.replaced) return "Signed in on another computer: a re-invite moved you there, and this computer stopped syncing. Your notes stay here. If you didn't do that, tell the campaign's owner.";
  if (sharing?.removed) return "Removed from this campaign. It no longer syncs; your notes stay here.";
  if (!sharing?.room) return "";
  if (!sharing.shared) return "Not syncing while Shared with my party is off.";
  const s = snap?.status;
  const text = !s || s.state === "connecting" ? "Connecting…"
    : s.state === "downloading" ? downloadText(s)
    : s.state === "syncing" ? (s.count ? `Syncing ${s.count}…` : "Syncing…")
    : s.state === "synced" ? "Synced"
    : s.state === "offline" ? "Offline, will sync when the server is back"
    : s.state === "removed" ? "Removed from this campaign"
    : s.state === "replaced" ? "Signed in on another computer"
    : s.state === "deleted" ? `The owner stopped sharing ${name}`
    : s.state === "queued" ? "Waiting: Lorekeeper syncs 8 shared campaigns at once. Open this one to sync it now"
    : `Sync stopped: ${s.reason}`;
  return snap?.warning ? `${text}. ${snap.warning}` : text;
}

/** "Downloading 120 of 340 files…" while a campaign catches up (older servers don't send the total). */
function downloadText({ done = 0, total }) {
  const files = (n) => `${n} ${n === 1 ? "file" : "files"}`;
  if (total) return `Downloading ${Math.min(done, total)} of ${files(total)}…`;
  return done ? `Downloading ${files(done)}…` : "Syncing…";
}

/**
 * What the Join dialog says, from the "sync-join" events ({ waiting, name }, shared.rs) and then the new campaign's sync
 * snapshot: "Joining Scale & Frost…", "Downloading 120 of 340 files…", "Done". `done` is true once it's all here.
 */
export function joinText(progress, snap) {
  const s = snap?.status;
  if (s?.state === "synced") return { text: "Done", done: true };
  if (s) return { text: syncText(snap, { shared: true, room: "joined" }), done: false };
  if (progress?.waiting && !progress.name) return { text: "Waiting for the owner's notes to upload…", done: false };
  return { text: progress?.name ? `Joining ${progress.name}…` : "Joining…", done: false };
}

/** A role as the players list and the campaign row show it. */
export const roleText = (role, manage) => (role === "owner" ? "owner" : role === "dm" ? (manage ? "DM who manages players" : "DM") : "player");

/** "Online: Lorelei, Syloth", or "" when nobody else is. */
export const onlineText = (snap) => (snap?.online?.length ? `Online: ${snap.online.join(", ")}` : "");

/** Someone came online or went offline between two snapshots: joining, leaving or removal show there first. */
export const onlineChanged = (before, after) => JSON.stringify(before?.online ?? []) !== JSON.stringify(after?.online ?? []);
