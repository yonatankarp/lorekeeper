// What a shared campaign's sync status says, in Settings and in the main window (shared.rs sends the snapshots).
// Everything here may come from teammates (names) or the server: callers set it as text, never as HTML.

/** The status line for a campaign's snapshot { status, online, warning } and its sharing settings. */
export function syncText(snap, sharing, open) {
  if (sharing?.replaced) return "Signed in on another computer: a re-invite moved you there, and this computer stopped syncing. Your notes stay here. If you didn't do that, tell the campaign's owner.";
  if (sharing?.removed) return "Removed from this campaign. It no longer syncs; your notes stay here.";
  if (!sharing?.room) return "";
  if (!sharing.shared) return "Not syncing while Shared with my party is off.";
  if (!open) return "Syncs while it's the open campaign.";
  const s = snap?.status;
  const text = !s || s.state === "connecting" ? "Connecting…"
    : s.state === "syncing" ? (s.count ? `Syncing ${s.count}…` : "Syncing…")
    : s.state === "synced" ? "Synced"
    : s.state === "offline" ? "Offline, will sync when the server is back"
    : s.state === "removed" ? "Removed from this campaign"
    : s.state === "replaced" ? "Signed in on another computer"
    : `Sync stopped: ${s.reason}`;
  return snap?.warning ? `${text}. ${snap.warning}` : text;
}

/** A role as the players list and the campaign row show it. */
export const roleText = (role, manage) => (role === "owner" ? "owner" : role === "dm" ? (manage ? "DM who manages players" : "DM") : "player");

/** "Online: Lorelei, Syloth", or "" when nobody else is. */
export const onlineText = (snap) => (snap?.online?.length ? `Online: ${snap.online.join(", ")}` : "");
