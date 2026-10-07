import { test } from "node:test";
import assert from "node:assert/strict";
import { onlineText, roleText, syncText } from "./sync-status.js";

const room = { shared: true, room: "abc", role: "member" };

test("sync status lines", () => {
  assert.equal(syncText(null, { shared: true }, true), "", "not synced yet");
  assert.equal(syncText(null, { ...room, removed: true, room: "" }, true), "Removed from this campaign. It no longer syncs; your notes stay here.");
  assert.equal(syncText(null, { ...room, shared: false }, true), "Not syncing while Shared with my party is off.");
  assert.equal(syncText(null, room, false), "Syncs while it's the open campaign.");
  assert.equal(syncText(null, room, true), "Connecting…");
  assert.equal(syncText({ status: { state: "syncing", count: 0 } }, room, true), "Syncing…");
  assert.equal(syncText({ status: { state: "syncing", count: 12 } }, room, true), "Syncing 12…");
  assert.equal(syncText({ status: { state: "synced" } }, room, true), "Synced");
  assert.equal(syncText({ status: { state: "offline" } }, room, true), "Offline, will sync when the server is back");
  assert.equal(syncText({ status: { state: "removed" } }, room, true), "Removed from this campaign");
  assert.equal(syncText({ status: { state: "replaced" } }, room, true), "Signed in on another computer");
  assert.equal(syncText(null, { ...room, removed: true, replaced: true, room: "" }, true), "Signed in on another computer: a re-invite moved you there, and this computer stopped syncing. Your notes stay here. If you didn't do that, tell the campaign's owner.");
  assert.equal(roleText("dm", true), "DM who manages players");
  assert.equal(roleText("dm", false), "DM");
  assert.equal(roleText("player", true), "player");
  assert.equal(roleText("owner", false), "owner");
  assert.equal(syncText({ status: { state: "stopped", reason: "Too big." } }, room, true), "Sync stopped: Too big.");
  assert.equal(syncText({ status: { state: "synced" }, warning: "A file is too big." }, room, true), "Synced. A file is too big.");
});

test("who's online", () => {
  assert.equal(onlineText({ online: [] }), "");
  assert.equal(onlineText(undefined), "");
  assert.equal(onlineText({ online: ["Lorelei", "Syloth"] }), "Online: Lorelei, Syloth");
});
