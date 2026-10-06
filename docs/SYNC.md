# Lorekeeper sync

Shared campaigns sync between the party's computers through a small Lorekeeper sync server. The server stores only encrypted data: it can't read note contents, file names or player names. Each player's local campaign folder stays the real copy; sync replicates it, and Lorekeeper works offline and catches up later.

The reference server runs on the maintainer's machine at `https://lorekeeper.yonatankarp.com`. The server address is a setting, so anyone can run their own from the published image (see "Running your own sync server").

## Pieces

| Path | What |
|---|---|
| `sync-protocol/` | Rust crate shared by the app and the server: message types, encryption, ids, invite links |
| `sync-server/` | The server (Rust, axum, SQLite), built into `ghcr.io/yonatankarp/lorekeeper-sync` |
| `src-tauri/` | The app; its sync engine uses `sync-protocol` |

`sync-protocol` is a path dependency of both (no Cargo workspace, so `src-tauri` builds as before).

## Rooms, owners, members and invites

A shared campaign that syncs is a **room** on the server.

- `room`: 16 random bytes, base32 (lowercase, no padding, 26 characters), public identifier.
- `key`: 32 random bytes, the room's encryption key. Made by the owner's app and never sent to the server.
- **Tokens**: 32 random bytes each, base64url without padding (43 characters). The server makes them and stores only `SHA-256(token)`, with a role and a `member_id` (16 random bytes, base32).
  - The **owner token** comes back when the room is created. Its holder, the room's creator, is the only one who can invite and remove players.
  - Each player gets their own **member token** by redeeming an invite. Owner and member tokens both sync; only the owner token administers.
- **Invite**: 16 random bytes, base32, made by the owner. One-time, expires after 7 days (`LOREKEEPER_SYNC_INVITE_DAYS`). At most 32 members plus pending invites per room.

**Invite link:** `https://lorekeeper.yonatankarp.com/join/<room>/<invite>#<key>` (key base64url, no padding). The part after `#` never reaches the server (browsers and HTTP clients don't send fragments), and no token travels in links. The server answers `GET /join/<room>/<invite>` with a small page saying to open the link in Lorekeeper (Settings > General > Join a shared campaign); showing the page doesn't use up the invite. The app accepts the full link pasted into its Join dialog, redeems the invite for its own member token, and talks to the server origin in the link.

Link parsing is strict (`Invite::parse`): `https://host[:port]` (plain `http://` only for `localhost`, `127.0.0.1` and `[::1]`), exactly `/join/<room>/<invite>`, no user info, query or trailing slash, and a 32-byte key. Surrounding whitespace is ignored and the host is lowercased.

Anyone with an unredeemed link can join once, so the owner shares it only with the party. Lorekeeper keeps `key` and the token in the OS keychain, never in settings files or notes.

**Member display id**: an opaque string a client chooses, sent when redeeming and in `hello`. Clients should send their PC name encrypted with the room key, so only the party can read it; the server only stores and relays it (up to 512 bytes).

## Encryption

- **File id:** `id = base32(BLAKE3-keyed(key, "lorekeeper file id v1" || path)[..16])`, where `path` is the file's path inside the campaign folder with `/` separators (e.g. `Sessions/Session 4/Sibling 5.md`) as UTF-8. Stable per path, unguessable without the key.
- **Blob:** `XChaCha20-Poly1305(key, random 24-byte nonce)` over the JSON `{"path": "...", "content": "<base64>", "modified": <unix ms>}` (fields in that order, `content` in standard base64 with padding); stored as `nonce || ciphertext || 16-byte tag`, and sent as standard base64 with padding. The AEAD's associated data is the room string followed by the id string (their 26 ASCII characters each), so a blob can't be moved to another file or room.
- **Encrypted tombstone:** a client deletes a file by writing a normal blob whose JSON is `{"path": "...", "content": "", "modified": <unix ms>, "deleted": true}` (`FileContent::tombstone`). The `deleted` field is left out of ordinary blobs, so their layout is unchanged. Clients never use the server's own delete (a null blob): it carries nothing sealed with the key, so anyone with a token could send one.
- **Member display id:** `base64(nonce || XChaCha20-Poly1305(key, nonce, name))` with associated data `room || "lorekeeper member name v1"` (`seal_member`, `open_member`): the player's character name, at most 300 bytes, "" for none.
- **Content hash** for change detection, computed by clients only: BLAKE3 of the plain bytes, lowercase hex.
- **Opening a blob** (`open`) also checks the path inside, since every party member holds the key and can seal anything: it must be relative and stay in the campaign folder (no empty, `.` or `..` components, no leading `/`, no `\`, NUL or drive prefix like `C:`), and the blob's file id must be the one derived from it. A blob that fails either check is treated like one that doesn't decrypt.
- **What the key doesn't protect:** the server can't check blobs, so anyone holding a token (a member, or someone who stole a token without the key) can overwrite a file with garbage or delete it (a tombstone carries no blob). They can't read or forge content. Clients should treat a blob that doesn't open as a conflict, not as new content.

`sync-protocol` has known-answer tests for the file id and the blob layout; changing either breaks every synced campaign.

## Sync channel (WebSocket)

`GET /v1/rooms/{room}/live` upgrades to a WebSocket. It is the one way clients read and write room contents: catch-up, live changes, writes and presence all go over it. The connection stays open; when it drops, the client reconnects and says `hello` with the last `seq` it has.

**Auth** on the upgrade request, owner or member token: `Authorization: Bearer <token>` (preferred). Clients that can't set headers on an upgrade (browsers) can instead offer the subprotocols `lorekeeper, token.<token>` (the server answers with `lorekeeper` and never echoes the token). A token in the URL (`?token=`) is never read, since URLs end up in proxy logs. A wrong, revoked or missing token, or an unknown room, gets `401` before the upgrade.

**Messages:** JSON text frames (types in `sync-protocol`: `ClientMessage`, `ServerMessage`). `seq` is one increasing counter per room; every successful write gets the next value.

Client to server:

| Message | |
|---|---|
| `{"type":"hello","since":N,"member":"<display id>"}` | Once, first. The server replays every file changed after `since` (0 for all) as `changes` batches, then sends `presence`, then streams live. |
| `{"type":"put","req":R,"id":"...","base":N,"blob":"<base64>"}` | Write a file. `base` = the `seq` the client last saw for this id (0 = new). `req` is any number the client picks to match the answer. |
| `{"type":"delete","req":R,"id":"...","base":N}` | Delete a file; it keeps a tombstone (blob null) so others learn of it. |
| `{"type":"ping"}` | Answered with `{"type":"pong"}`. |

Server to client:

| Message | |
|---|---|
| `{"type":"changes","seq":S,"changes":[{"id","seq","blob" or null}],"more":bool}` | Replay batch after `hello` (`seq` = the room's latest). At most 500 changes and 4 MiB of blobs per batch (always at least one change); the last batch has `more: false`. Each file appears once, at its latest version. |
| `{"type":"change","id":"...","seq":S,"blob":"..." or null}` | A write by another connection, sent to every connected socket of the room, including the writer's other devices, but not to the socket that wrote it (it got an `ack`). |
| `{"type":"ack","req":R,"seq":S}` | The write is stored as `seq` S. |
| `{"type":"conflict","req":R,"seq":S,"blob":"..." or null}` | The file changed since `base`: its current version (null when deleted), for the client to resolve. |
| `{"type":"presence","members":[{"member_id","member","role"}]}` | Who is connected (after their `hello`), one entry per member; sent after the replay and whenever it changes. |
| `{"type":"error","req":R,"error":"<code>"}` | A refused message; `req` is there when the message had one. The socket stays open. |

**Write rules:** a write succeeds when `base` equals the file's current `seq`; for a file that doesn't exist or is deleted, `base` 0 also matches, so a file can be created again over a tombstone. Deleting a missing or already deleted file changes nothing and answers `ack` with that file's current `seq` (0 if it never existed). `put` and `delete` before `hello` get `hello_first`.

**Error codes:** `bad_message`, `hello_first`, `hello_twice`, `member_too_long`, `bad_id`, `bad_blob` (not base64, or shorter than nonce and tag), `too_large`, `room_full`, `too_many_files`, `rate_limited`, `internal`.

**Keeping it open:** the server sends a WebSocket ping every 30 seconds (Cloudflare drops connections idle for about 100 seconds) and closes a socket it hasn't heard from (any frame, pongs included) in 75 seconds. It closes sockets with code `4001` when their member is removed (at once, even in the middle of a replay or a send) and, when it gets the chance, `1012` when the server shuts down; clients reconnect after `1012` or a dropped connection, and after `4001` tell the player they were removed. Frames are limited to the blob limit in base64 plus 64 KiB; a bigger frame closes the socket. More than 600 client messages in a minute on one socket close it with `1008`. A socket that doesn't take a frame within 5 minutes (a reader that stopped reading) is dropped.

## HTTP API (v1)

Plain HTTP covers room creation, invites and members, health, the join page and an optional read. All JSON; errors are `{"error":"<code>"}`. Auth: `Authorization: Bearer <token>`; a wrong, revoked or missing token gets `401 {"error":"unauthorized"}`, the same answer for unknown rooms, so rooms can't be probed. Owner-only endpoints answer a member token with `403 {"error":"owner_only"}`. Times are unix milliseconds.

| Method and path | Auth | Answer |
|---|---|---|
| `POST /v1/rooms` | creation key | `201 {"room","owner_token"}`. Needs the header `X-Lorekeeper-Create-Key: <key>` (the server's `LOREKEEPER_SYNC_CREATE_KEY`), else `403 {"error":"create_key_required"}` (the app then asks for it). Rate limited per client IP, counting only requests with the right key. |
| `POST /v1/rooms/{room}/invites` | owner | `201 {"invite","expires"}`; `413 too_many_members` at 32 members plus pending invites. |
| `GET /v1/rooms/{room}/invites` | owner | `200 [{"invite","expires"}]`, pending invites only. |
| `DELETE /v1/rooms/{room}/invites/{invite}` | owner | `204`; `404 invite_not_found` when it isn't pending. |
| `POST /v1/rooms/{room}/invites/{invite}/redeem` | none | Body `{"member":"<display id>"}`. `201 {"token"}`, a new member token; uses up the invite. `404 invite_not_found` (unknown or revoked invite, or unknown room), `410 invite_used`, `410 invite_expired`. Rate limited per client IP. |
| `GET /v1/rooms/{room}/members` | owner | `200 [{"member_id","member","role","created","last_seen"}]`; `last_seen` (null until then) is bumped when a socket connects, `member` by `hello`. |
| `DELETE /v1/rooms/{room}/members/{member_id}` | owner | `204`; the token stops working at once and the member's sockets close. `400 cannot_remove_owner`, `404 not_found`. |
| `GET /v1/rooms/{room}/changes?since=N` | owner or member | `200 {"seq","changes","more"}`, the same page as a replay batch, without waiting. For scripts, debugging and tests; the app uses the socket. Shares the per-member read budget with socket upgrades. |
| `GET /v1/rooms/{room}/live` | owner or member | WebSocket, see above. |
| `GET /join/{room}/{invite}` | none | The explanation page (static: it shows neither the room nor the invite, loads nothing, and is served with a `default-src 'none'` CSP that also forbids framing). |
| `GET /healthz` | none | `200 ok` (for the container healthcheck) |
| `GET /` | none | redirect to `https://yonatankarp.com/lorekeeper/` |

## Limits

Configurable by environment; defaults: blob 30 MiB (the decoded `nonce || ciphertext`; in base64 on the wire about 40 MiB, so frames are allowed the blob limit in base64 plus 64 KiB), room total 1 GiB of blobs, 20,000 files per room (tombstones don't count, but a new file is refused once the room holds twice that many files and tombstones together), 60 writes per minute per room, 32 connections per room and 64 on the whole server, room creation 5 per hour per IP, invite redemption 20 per hour per IP, 32 members plus pending invites per room. A room over a lowered limit can still shrink. Over a limit: `413` or `429` with `{"error": "..."}` over HTTP (`too_many_connections` on the upgrade), or an `error` frame on the socket.

Fixed, not configurable: 60 socket upgrades plus `GET /changes` per member per minute (each can read the whole room), 600 messages per socket per minute, 4 MiB of blobs per page of changes. Per-IP limits count an IPv6 client as its /64, and each limit tracks at most 10,000 clients at once (past that, new clients get `429` until a window ends). Each member (all their devices together) can hold at most 8 sockets. There is no per-IP connection cap: a socket needs a token, so the per-member, per-room and server-wide caps bound what one party can hold.

**Memory:** a socket sending a page holds it about three times (the blobs, their base64, the JSON frame), and one receiving a `put` holds the frame about three times too. A page is at most 4 MiB of blobs unless a single blob is bigger, so the worst case is about 3 x the blob limit per busy socket: around 100 MB at the defaults, for example when one 30 MiB file fans out to every player at once. Size the container's memory for the largest files your party shares times the sockets that receive them at once, or lower `LOREKEEPER_SYNC_MAX_BLOB`.

The server logs no blobs, tokens, keys, invites, display ids or file ids: only its start, its shutdown and database errors.

## Server configuration

| Env | Default | |
|---|---|---|
| `LOREKEEPER_SYNC_DB` | `/data/sync.db` | SQLite file (WAL mode; its `-wal` and `-shm` files live next to it) |
| `LOREKEEPER_SYNC_ADDR` | `0.0.0.0:8080` | listen address |
| `LOREKEEPER_SYNC_PUBLIC_URL` | `https://lorekeeper.yonatankarp.com` | shown on the join page |
| `LOREKEEPER_SYNC_TRUST_PROXY` | `none` | `none`: rate limits use the connection's address; `cloudflare`: read the client IP from `CF-Connecting-IP`. Only use `cloudflare` when nothing but cloudflared can reach the port (for example listening on `127.0.0.1` in cloudflared's network namespace): anyone who reaches it directly can send the header and dodge the per-IP limits. |
| `LOREKEEPER_SYNC_CREATE_KEY` | required | `POST /v1/rooms` needs it in `X-Lorekeeper-Create-Key`. The server refuses to start without one, or with one under 32 characters. Used exactly as given (`/`, `+`, `=` are fine). Never logged. To run an open server, publish the key. |
| `LOREKEEPER_SYNC_INVITE_DAYS` | `7` | invite lifetime |
| limit overrides | | `LOREKEEPER_SYNC_MAX_BLOB`, `..._MAX_ROOM_BYTES` (bytes), `..._MAX_FILES`, `..._WRITES_PER_MINUTE`, `..._MAX_CONNECTIONS` (per room), `..._MAX_CONNECTIONS_TOTAL` (server-wide), `..._ROOMS_PER_HOUR`, `..._REDEEMS_PER_HOUR` |

Container: listens on 8080, data in the `/data` volume, runs as UID and GID 10001 (no home directory or passwd entry needed), writes nothing outside `/data` (so `read_only: true` works), healthcheck `lorekeeper-sync --healthcheck` (GETs `/healthz` on the port from `LOREKEEPER_SYNC_ADDR`, exit 0 or 1; the image has no shell or curl). Image: `ghcr.io/yonatankarp/lorekeeper-sync` (linux/amd64), tags `sha-<short commit>` and `latest` from main, built by `.github/workflows/sync-server.yml` when `sync-server/` or `sync-protocol/` change.

## Running your own sync server

```bash
openssl rand -base64 33 > create-key      # 44 characters; keep it private
docker volume create lorekeeper-sync
docker run -d --name lorekeeper-sync --restart unless-stopped \
  -p 8080:8080 -v lorekeeper-sync:/data \
  --read-only --tmpfs /tmp \
  -e LOREKEEPER_SYNC_PUBLIC_URL=https://sync.example.com \
  -e LOREKEEPER_SYNC_TRUST_PROXY=none \
  -e LOREKEEPER_SYNC_CREATE_KEY="$(cat create-key)" \
  ghcr.io/yonatankarp/lorekeeper-sync:latest
```

- Put it behind HTTPS (a reverse proxy, or a Cloudflare tunnel with `LOREKEEPER_SYNC_TRUST_PROXY=cloudflare`), and make sure the proxy passes WebSocket upgrades. Invite links only accept `https://` for anything but localhost.
- **`LOREKEEPER_SYNC_CREATE_KEY` is required.** The app is public, so without it anyone who knows the address could create rooms on your server and fill your disk. The owner enters the key once when creating a shared campaign; players joining with an invite never need it.
- Exposed directly (no proxy), the server has no header-read timeout, so slow clients can hold connections open; a reverse proxy or a Cloudflare tunnel in front takes care of that.
- With a bind mount instead of a named volume, make the directory writable by UID 10001 (`chown 10001:10001 /srv/lorekeeper-sync`).
- Back up the volume; `sync.db` with its `-wal` file is the whole state. It holds only encrypted data, but losing it means every player uploads again.
- In Lorekeeper, set **Settings > General > Sync server** to your URL before sharing the campaign (players who join get the server from their invite link).

## Client behaviour (app)

The engine is `src-tauri/src/sync.rs`; it knows nothing of Tauri and is tested end to end against the real server in `sync_tests.rs`. `src-tauri/src/shared.rs` wires it to the app.

- **Settings and secrets:** per campaign, `sharing[<folder>]` holds `shared`, `me` (your PC), `server`, `room`, `role` (`owner` or `member`) and `removed`. Only Share and Join set the last four; the settings window can't change them. The room key and the campaign's token are one keychain entry (`sync-room <room>`), and the server's creation key another (`sync-create-key <server origin>`), sent only to that server. Nothing secret goes into settings, state files, logs, events or error messages. The server for new shared campaigns is the **Sync server** setting, else `LOREKEEPER_SYNC_SERVER`, else `https://lorekeeper.yonatankarp.com`; any of them must pass `server_origin` (https, or http only for loopback).
- **Share** (owner): `POST /v1/rooms` with the creation key from the keychain; on `create_key_required` the window asks for it and Lorekeeper keeps it once it works. A new random key goes to the keychain with the owner token, and the campaign's name goes into `.lorekeeper/campaign.json` (`{"name": ...}`), the one hidden file that syncs, so joiners can name their folder. Then the first sync uploads everything.
- **Invite** (owner): one-time invites, each link built with `Invite::link` only when the owner clicks, shown once in a dialog (Copy copies; the dialog clears them on close or after 10 minutes). Pending invites can be cancelled, and players (names decrypted, last seen) removed.
- **Join:** the pasted link is parsed (`Invite::parse`), the window shows its server (and warns when it isn't the default) and the player confirms. Then redeem (display id "" until a PC is picked), keep key and token in the keychain, read the replay just until `campaign.json` to learn the name, check it as a folder name, and make `<library>/<name>` (" 2" and so on when taken). The campaign is added with shared on and role member, opened (which starts the download), and Settings asks which PC you play.
- **Only the open campaign syncs.** Switching to another shared campaign disconnects and connects that one, which catches up from its last `seq`. Choosing your PC reconnects, so `hello` carries the new display id.
- **What syncs:** Markdown pages and images (the app's list: png, jpg, jpeg, gif, webp, svg) under names that are safe on every system (no Windows-reserved names or characters, no control characters, no trailing dot or space, at most 255 bytes a name and 1024 a path), plus `.lorekeeper/campaign.json`. Never: other hidden files and folders (`.obsidian`, `.trash`, `.DS_Store`), the top-level `Templates/` folder (Lorekeeper moves a campaign's templates into the shared Lorekeeper folder, which would read as deletions), sync conflict copies (the named patterns `syncConflicts` in vault.js flags), symlinks, files over about 22 MB (a warning), or anything outside the campaign folder. The same check applies to every path that arrives.
- **State:** `<config>/sync/<room>.json`, written atomically: the room's last `seq` (from `changes` and `change` only, never from an `ack`) and `path -> {id, seq, hash, len, gone}`. A state file that's damaged or doesn't fit the key means a fresh catch-up: local files are compared by hash and never deleted.
- **Connect:** `Authorization: Bearer` on the upgrade (wss with rustls and the Mozilla roots, ws only for loopback), frames capped at the server's limit. `hello` with the last `seq`, apply the replay pages (`more: true` loops), then push. A ping every 30 seconds; nothing heard for 75 seconds (sleep, a dead network) means reconnect. Reconnects back off from 1 second, doubling to a minute; only a connection that lasted a minute starts over, so a server that keeps closing or refusing (the per-member connection budget) isn't hammered. `401` on the upgrade or close code `4001` mean removed: sync stops for good, the campaign is marked no longer synced, its token and state are deleted, and the files stay.
- **Push:** the 3-second folder watch (which sees the app's own writes too) pokes the engine, which compares each file's hash with the state (sizes and times cached, so unchanged files aren't read again). A changed file is a `put` with `base` = its last seen `seq`; a deleted one an encrypted tombstone. At most 4 writes are in flight; `rate_limited` pauses 20 seconds and keeps the queue; `too_large` and other refusals skip that version with a warning; `room_full` and `too_many_files` stop pushing and say so. If the campaign folder is missing, or its synced `campaign.json` is gone, deletions aren't pushed: an unmounted drive must never delete everyone's files.
- **Pull:** for each change, a version already applied (`seq` not above the one seen for that id) is skipped, so a live change and a `conflict` reply for the same version count once. A blob is decrypted and checked (`open` checks path and id; then the path must sync). If the local file is unchanged since it last synced (or absent), the new content is written through a temporary file and a rename in its folder, after checking that no folder on the way and not the target is a symlink and that the result stays under the campaign folder. If it also changed locally, the local file stays and the incoming version is saved beside it as `<name> (conflict <date> <time>).<ext>` (a counter when taken, created with `create_new`), which Home's Sync conflicts card lists; the local version then goes up over it. A tombstone moves an unchanged file into the campaign's hidden `.trash` folder (a changed one stays and goes up again). Writes are recorded as the app's own, so the folder watch doesn't report them, and the windows refresh.
- **Untrusted versions:** a server-side delete (null blob) or a blob that doesn't open never touches local files; the player gets a warning, and the local version goes back up over it, which repairs the room for everyone.
- **Limits:** more than 20,000 files or 1 GiB in a room, more than 4 GiB written or 500 conflict copies in one run stop sync with a message, so a hostile server can't fill the disk.
- **Status** (events to the windows and on the campaign in Settings and the sidebar): "Connecting", "Syncing" or "Syncing N", "Synced", "Offline, will sync when the server is back", "Removed from this campaign", or "Sync stopped" with the reason. **Presence:** the names from `presence`, decrypted, without your own entry (told apart by your sealed display id).
- **Leaving:** removing a joined campaign from Lorekeeper deletes its token and state. The owner's secrets stay when they remove their own campaign: they're the only way to invite or remove players.
- **Renames:** ids come from paths, so a rename syncs as a new file plus a tombstone for the old path. (A `moved_to` blob that also moves private twins is planned with private notes.)
- **Per-player session files** mean the common case, everyone writing notes during a game, never conflicts.

## Private notes (planned)

Each player's private notes live in a private folder outside the campaign folder, so sync never sees them. A private page is the twin of a shared page at the same path: `NPCs/Mirela.md` in the private folder holds your own notes about the shared `NPCs/Mirela.md`, and the app shows the two as one page (the shared text, then your notes with a lock). A private file without a shared twin is a private page of its own. Same path means no ids or links to keep in step, and each folder stays plain Markdown that Obsidian can open. Renaming a page in the app moves both; a rename by another player arrives as `moved_to` (see Renames) and moves your twin too.
