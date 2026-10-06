import { test } from "node:test";
import assert from "node:assert/strict";
import { onlineText, syncText } from "./sync-status.js";

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
  assert.equal(syncText({ status: { state: "stopped", reason: "Too big." } }, room, true), "Sync stopped: Too big.");
  assert.equal(syncText({ status: { state: "synced" }, warning: "A file is too big." }, room, true), "Synced. A file is too big.");
});

test("who's online", () => {
  assert.equal(onlineText({ online: [] }), "");
  assert.equal(onlineText(undefined), "");
  assert.equal(onlineText({ online: ["Lorelei", "Syloth"] }), "Online: Lorelei, Syloth");
});
