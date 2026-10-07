import { test } from "node:test";
import assert from "node:assert/strict";
import { joinText, onlineText, roleText, syncText } from "./sync-status.js";

const room = { shared: true, room: "abc", role: "member" };

test("sync status lines", () => {
  assert.equal(syncText(null, { shared: true }, true), "", "not synced yet");
  assert.equal(syncText(null, { ...room, removed: true, room: "" }, true), "Removed from this campaign. It no longer syncs; your notes stay here.");
  assert.equal(syncText(null, { ...room, shared: false }, true), "Not syncing while Shared with my party is off.");
  assert.equal(syncText(null, room), "Connecting…", "open or not: every shared campaign syncs");
  assert.equal(syncText({ status: { state: "downloading", done: 0, total: null } }, room), "Syncing…");
  assert.equal(syncText({ status: { state: "downloading", done: 120, total: 340 } }, room), "Downloading 120 of 340 files…");
  assert.equal(syncText({ status: { state: "downloading", done: 1, total: 1 } }, room), "Downloading 1 of 1 file…");
  assert.equal(syncText({ status: { state: "downloading", done: 345, total: 340 } }, room), "Downloading 340 of 340 files…", "never past the total");
  assert.equal(syncText({ status: { state: "downloading", done: 12, total: null } }, room), "Downloading 12 files…", "an older server sends no total");
  assert.equal(syncText({ status: { state: "downloading", done: 0, total: 0 } }, room), "Syncing…");
  assert.equal(syncText({ status: { state: "queued" } }, room), "Waiting: Lorekeeper syncs 8 shared campaigns at once. Open this one to sync it now");
  assert.equal(syncText({ status: { state: "syncing", count: 0 } }, room, true), "Syncing…");
  assert.equal(syncText({ status: { state: "syncing", count: 12 } }, room, true), "Syncing 12…");
  assert.equal(syncText({ status: { state: "synced" } }, room, true), "Synced");
  assert.equal(syncText({ status: { state: "offline" } }, room, true), "Offline, will sync when the server is back");
  assert.equal(syncText({ status: { state: "removed" } }, room, true), "Removed from this campaign");
  assert.equal(syncText({ status: { state: "replaced" } }, room, true), "Signed in on another computer");
  assert.equal(syncText(null, { ...room, removed: true, replaced: true, room: "" }, true), "Signed in on another computer: a re-invite moved you there, and this computer stopped syncing. Your notes stay here. If you didn't do that, tell the campaign's owner.");
  // The owner stopped sharing (close code 4003): the line names the campaign, before and after the settings say so.
  assert.equal(syncText({ status: { state: "deleted" } }, room, "Curse of Strahd"), "The owner stopped sharing Curse of Strahd");
  assert.equal(syncText(null, { ...room, removed: true, unshared: true, room: "" }, "Curse of Strahd"), "The owner stopped sharing Curse of Strahd. Your copy stays on this computer.");
  assert.equal(syncText(null, { ...room, removed: true, unshared: true, room: "" }), "The owner stopped sharing this campaign. Your copy stays on this computer.");
  assert.equal(roleText("dm", true), "DM who manages players");
  assert.equal(roleText("dm", false), "DM");
  assert.equal(roleText("player", true), "player");
  assert.equal(roleText("owner", false), "owner");
  assert.equal(syncText({ status: { state: "stopped", reason: "Too big." } }, room, true), "Sync stopped: Too big.");
  assert.equal(syncText({ status: { state: "synced" }, warning: "A file is too big." }, room, true), "Synced. A file is too big.");
});

test("join progress", () => {
  assert.deepEqual(joinText(null, null), { text: "Joining…", done: false });
  assert.deepEqual(joinText({ waiting: true, name: "" }, null), { text: "Waiting for the owner's notes to upload…", done: false });
  assert.deepEqual(joinText({ waiting: false, name: "Scale & Frost" }, null), { text: "Joining Scale & Frost…", done: false });
  const snap = (status) => ({ status, online: [], warning: "" });
  assert.deepEqual(joinText({ name: "Scale & Frost" }, snap({ state: "connecting" })), { text: "Connecting…", done: false });
  assert.deepEqual(joinText({ name: "Scale & Frost" }, snap({ state: "downloading", done: 120, total: 340 })), { text: "Downloading 120 of 340 files…", done: false });
  assert.deepEqual(joinText({ name: "Scale & Frost" }, snap({ state: "synced" })), { text: "Done", done: true });
  assert.deepEqual(joinText({ name: "Scale & Frost" }, snap({ state: "stopped", reason: "The campaign folder isn't there." })),
    { text: "Sync stopped: The campaign folder isn't there.", done: false });
});

test("who's online", () => {
  assert.equal(onlineText({ online: [] }), "");
  assert.equal(onlineText(undefined), "");
  assert.equal(onlineText({ online: ["Lorelei", "Syloth"] }), "Online: Lorelei, Syloth");
});
