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
- **Content hash** for change detection, computed by clients only: BLAKE3 of the plain bytes, lowercase hex.

`sync-protocol` has known-answer tests for the file id and the blob layout; changing either breaks every synced campaign.

## Sync channel (WebSocket)

`GET /v1/rooms/{room}/live` upgrades to a WebSocket. It is the one way clients read and write room contents: catch-up, live changes, writes and presence all go over it. The connection stays open; when it drops, the client reconnects and says `hello` with the last `seq` it has.

**Auth** on the upgrade request, owner or member token: `Authorization: Bearer <token>` (preferred). Clients that can't set headers on an upgrade (browsers) can instead offer the subprotocols `lorekeeper, token.<token>` (the server answers with `lorekeeper` and never echoes the token) or add `?token=<token>` (last resort: URLs end up in logs). A wrong, revoked or missing token, or an unknown room, gets `401` before the upgrade.

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
| `{"type":"changes","seq":S,"changes":[{"id","seq","blob" or null}],"more":bool}` | Replay batch after `hello` (`seq` = the room's latest). At most 500 changes and about one blob limit of data per batch (always at least one change); the last batch has `more: false`. Each file appears once, at its latest version. |
| `{"type":"change","id":"...","seq":S,"blob":"..." or null}` | A write by another connection, sent to every connected socket of the room, including the writer's other devices, but not to the socket that wrote it (it got an `ack`). |
| `{"type":"ack","req":R,"seq":S}` | The write is stored as `seq` S. |
| `{"type":"conflict","req":R,"seq":S,"blob":"..." or null}` | The file changed since `base`: its current version (null when deleted), for the client to resolve. |
| `{"type":"presence","members":[{"member_id","member","role"}]}` | Who is connected (after their `hello`), one entry per member; sent after the replay and whenever it changes. |
| `{"type":"error","req":R,"error":"<code>"}` | A refused message; `req` is there when the message had one. The socket stays open. |

**Write rules:** a write succeeds when `base` equals the file's current `seq`; for a file that doesn't exist or is deleted, `base` 0 also matches, so a file can be created again over a tombstone. Deleting a missing or already deleted file changes nothing and answers `ack` with that file's current `seq` (0 if it never existed). `put` and `delete` before `hello` get `hello_first`.

**Error codes:** `bad_message`, `hello_first`, `hello_twice`, `member_too_long`, `bad_id`, `bad_blob` (not base64, or shorter than nonce and tag), `too_large`, `room_full`, `too_many_files`, `rate_limited`, `internal`.

**Keeping it open:** the server sends a WebSocket ping every 30 seconds (Cloudflare drops connections idle for about 100 seconds) and closes a socket it hasn't heard from (any frame, pongs included) in 75 seconds. It closes sockets with code `4001` when their member is removed and, when it gets the chance, `1012` when the server shuts down; clients reconnect after `1012` or a dropped connection, and after `4001` tell the player they were removed. Frames are limited to the blob limit in base64 plus 64 KiB; a bigger frame closes the socket.

## HTTP API (v1)

Plain HTTP covers room creation, invites and members, health, the join page and an optional read. All JSON; errors are `{"error":"<code>"}`. Auth: `Authorization: Bearer <token>`; a wrong, revoked or missing token gets `401 {"error":"unauthorized"}`, the same answer for unknown rooms, so rooms can't be probed. Owner-only endpoints answer a member token with `403 {"error":"owner_only"}`. Times are unix milliseconds.

| Method and path | Auth | Answer |
|---|---|---|
| `POST /v1/rooms` | creation key, when the server has one | `201 {"room","owner_token"}`. With `LOREKEEPER_SYNC_CREATE_KEY` set, needs the header `X-Lorekeeper-Create-Key: <key>`, else `403 {"error":"create_key_required"}` (the app then asks for it). Rate limited per client IP. |
| `POST /v1/rooms/{room}/invites` | owner | `201 {"invite","expires"}`; `413 too_many_members` at 32 members plus pending invites. |
| `GET /v1/rooms/{room}/invites` | owner | `200 [{"invite","expires"}]`, pending invites only. |
| `DELETE /v1/rooms/{room}/invites/{invite}` | owner | `204`; `404 invite_not_found` when it isn't pending. |
| `POST /v1/rooms/{room}/invites/{invite}/redeem` | none | Body `{"member":"<display id>"}`. `201 {"token"}`, a new member token; uses up the invite. `404 invite_not_found` (unknown or revoked invite, or unknown room), `410 invite_used`, `410 invite_expired`. Rate limited per client IP. |
| `GET /v1/rooms/{room}/members` | owner | `200 [{"member_id","member","role","created","last_seen"}]`; `last_seen` (null until then) is bumped when a socket connects, `member` by `hello`. |
| `DELETE /v1/rooms/{room}/members/{member_id}` | owner | `204`; the token stops working at once and the member's sockets close. `400 cannot_remove_owner`, `404 not_found`. |
| `GET /v1/rooms/{room}/changes?since=N` | owner or member | `200 {"seq","changes","more"}`, the same page as a replay batch, without waiting. For scripts, debugging and tests; the app uses the socket. |
| `GET /v1/rooms/{room}/live` | owner or member | WebSocket, see above. |
| `GET /join/{room}/{invite}` | none | The explanation page. |
| `GET /healthz` | none | `200 ok` (for the container healthcheck) |
| `GET /` | none | redirect to `https://yonatankarp.com/lorekeeper/` |

## Limits

Configurable by environment; defaults: blob 30 MiB (the decoded `nonce || ciphertext`; in base64 on the wire about 40 MiB, so frames are allowed the blob limit in base64 plus 64 KiB), room total 1 GiB of blobs, 20,000 files per room (tombstones don't count), 60 writes per minute per room, 32 connections per room, room creation 5 per hour per IP, invite redemption 20 per hour per IP, 32 members plus pending invites per room. A room over a lowered limit can still shrink. Over a limit: `413` or `429` with `{"error": "..."}` over HTTP (`too_many_connections` on the upgrade), or an `error` frame on the socket.

The server logs no blobs, tokens, keys, invites, display ids or file ids: only its start, its shutdown and database errors.

## Server configuration

| Env | Default | |
|---|---|---|
| `LOREKEEPER_SYNC_DB` | `/data/sync.db` | SQLite file (WAL mode; its `-wal` and `-shm` files live next to it) |
| `LOREKEEPER_SYNC_ADDR` | `0.0.0.0:8080` | listen address |
| `LOREKEEPER_SYNC_PUBLIC_URL` | `https://lorekeeper.yonatankarp.com` | shown on the join page |
| `LOREKEEPER_SYNC_TRUST_PROXY` | `cloudflare` | `cloudflare`: read the client IP from `CF-Connecting-IP` (behind cloudflared); `none`: use the connection's address |
| `LOREKEEPER_SYNC_CREATE_KEY` | unset | When set, `POST /v1/rooms` needs it in `X-Lorekeeper-Create-Key`. At least 32 characters (the server refuses to start otherwise), used exactly as given (`/`, `+`, `=` are fine); empty means unset. Never logged. |
| `LOREKEEPER_SYNC_INVITE_DAYS` | `7` | invite lifetime |
| limit overrides | | `LOREKEEPER_SYNC_MAX_BLOB`, `..._MAX_ROOM_BYTES` (bytes), `..._MAX_FILES`, `..._WRITES_PER_MINUTE`, `..._MAX_CONNECTIONS` (per room), `..._ROOMS_PER_HOUR`, `..._REDEEMS_PER_HOUR` |

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
- **Set `LOREKEEPER_SYNC_CREATE_KEY` for a private server.** The app is public, so without it anyone who knows the address can create rooms on your server (rate limited, but still your disk). The owner enters the key once when creating a shared campaign; players joining with an invite never need it.
- With a bind mount instead of a named volume, make the directory writable by UID 10001 (`chown 10001:10001 /srv/lorekeeper-sync`).
- Back up the volume; `sync.db` with its `-wal` file is the whole state. It holds only encrypted data, but losing it means every player uploads again.
- In Lorekeeper, set the server address to your URL before creating the shared campaign.

## Client behaviour (app)

- A campaign syncs when it's **Shared with my party** and has a room. The owner creates the room (asking for the server's creation key if it answers `create_key_required`) and makes one invite link per player from **Create invite link**; a player joins from a link. Settings keep `server` and `room` per campaign; `key` and the campaign's token (owner or member) go to the keychain. The owner sees the members and can remove one.
- **What syncs:** every file in the campaign folder except hidden files and folders (`.obsidian`, `.DS_Store`, `.trash`), sync conflict copies, and the private folder (which lives outside the campaign folder anyway). Images and portraits sync too.
- **State:** per campaign, a local file in the app's config folder maps `path -> {id, seq, hash}` and stores the room's last `seq`.
- **Connect:** open the socket, `hello` with the last `seq`, apply the replay, then stay connected. Reconnect with backoff when it drops.
- **Pull:** for each change (replay or live): decrypt; if the local file is unchanged since it last synced (hash matches state) or absent, write the new content (or delete it); if it also changed locally, keep the local file and save the incoming version beside it as `<name> (conflict <date>).md` (flagged on Home's Sync conflicts card).
- **Push:** the app's own writes and the 3-second folder watch mark changed paths; each is sent as `put` or `delete` with `base` = its last seen `seq`. On `conflict`, apply the pull rule to the returned version.
- **Presence:** the members list from `presence`, names decrypted with the room key.
- **Per-player session files** mean the common case, everyone writing notes during a game, never conflicts.
- Offline or server down: changes queue (they're just files that differ from the state), and sync resumes when the server answers. Status in Settings: "Synced", "Syncing...", "Offline, will sync when the server is back".
