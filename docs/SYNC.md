# Lorekeeper sync

Shared campaigns sync between the party's computers through a small Lorekeeper sync server. The server stores only encrypted data: it can't read note contents or file names. Each player's local campaign folder stays the real copy; sync replicates it, and Lorekeeper works offline and catches up later.

The reference server runs on the maintainer's machine at `https://lorekeeper.yonatankarp.com`. The server address is a setting, so anyone can run their own from the published image.

## Pieces

| Path | What |
|---|---|
| `sync-protocol/` | Rust crate shared by the app and the server: message types, encryption, ids, invite links |
| `sync-server/` | The server (Rust, axum, SQLite), built into `ghcr.io/yonatankarp/lorekeeper-sync` |
| `src-tauri/` | The app; its sync engine uses `sync-protocol` |

`sync-protocol` is a path dependency of both (no Cargo workspace, so `src-tauri` builds as before).

## Rooms, keys and invites

A shared campaign that syncs is a **room** on the server.

- `room`: 16 random bytes, base32 (lowercase, no padding), public identifier.
- `key`: 32 random bytes, the room's encryption key. Never sent to the server.
- `token`: 32 random bytes, the room's write/read credential. The server stores only `SHA-256(token)`.

**Invite link:** `https://lorekeeper.yonatankarp.com/join/<room>#<key>.<token>` (key and token base64url, no padding). The part after `#` never reaches the server (browsers and HTTP clients don't send fragments). The server answers `GET /join/<room>` with a small page saying "Open this link in Lorekeeper: Settings > General > Join a shared campaign" so a link opened in a browser explains itself. The app accepts the full link pasted into its Join dialog and talks to the server origin in the link.

Anyone with the link can read and write the room: it's for the party. Lorekeeper keeps `key` and `token` in the OS keychain, never in settings files or notes.

## Encryption

- **File id:** `id = base32(BLAKE3-keyed(key, "lorekeeper file id v1" || path)[..16])`, where `path` is the file's path inside the campaign folder with `/` separators (e.g. `Sessions/Session 4/Sibling 5.md`). Stable per path, unguessable without the key.
- **Blob:** `XChaCha20-Poly1305(key, random 24-byte nonce)` over the JSON `{"path": "...", "content": "<base64>", "modified": <unix ms>}`; stored as `nonce || ciphertext`, base64 on the wire. The AEAD's associated data is `room || id`, so a blob can't be moved to another file or room.
- **Content hash** for change detection, computed by clients only: BLAKE3 of the plain bytes.

## HTTP API (v1)

All JSON. Auth on room endpoints: `Authorization: Bearer <token base64url>`; wrong or missing token: 401 (same answer for unknown rooms, so rooms can't be probed).

| Method and path | Body / query | Answer |
|---|---|---|
| `POST /v1/rooms` | none | `201 {"room", "token"}`. Rate limited per client IP (e.g. 5 per hour); the server makes the token. |
| `GET /v1/rooms/{room}/changes?since=N&wait=S` | `since` = last `seq` seen (0 for all); `wait` up to 25 seconds | `200 {"seq": latest, "changes": [{"id", "seq", "blob" or null when deleted}]}`. With `wait`, the server holds the request until something newer than `since` exists or the wait ends (long poll). At most 500 changes per answer; ask again with the last `seq` when `more: true`. |
| `PUT /v1/rooms/{room}/files/{id}` | `{"base": seq the client last saw for this id (0 = new), "blob"}` | `200 {"seq"}`; `409 {"seq", "blob"}` when the file changed since `base` (the current version, for the client to resolve). |
| `DELETE /v1/rooms/{room}/files/{id}?base=N` | none | `200 {"seq"}`; `409` as above. Deletion keeps a tombstone (blob null) so others learn of it. |
| `GET /healthz` | none | `200 ok` (for the container healthcheck) |
| `GET /` | none | redirect to `https://yonatankarp.com/lorekeeper/` |

`seq` is one increasing counter per room; every successful write gets the next value.

**Limits** (configurable by environment, defaults): blob 30 MB, room total 1 GB, 20,000 files per room, request body 32 MB, 60 writes per minute per room, room creation 5 per hour per IP. Over a limit: `413` or `429` with a short JSON `{"error": "..."}`. The server logs no blobs, tokens or ids beyond the room.

## Server configuration

| Env | Default | |
|---|---|---|
| `LOREKEEPER_SYNC_DB` | `/data/sync.db` | SQLite file (WAL mode) |
| `LOREKEEPER_SYNC_ADDR` | `0.0.0.0:8080` | listen address |
| `LOREKEEPER_SYNC_PUBLIC_URL` | `https://lorekeeper.yonatankarp.com` | for the join page |
| `LOREKEEPER_SYNC_TRUST_PROXY` | `cloudflare` | read the client IP from `CF-Connecting-IP` (behind cloudflared) |
| limit overrides | | `LOREKEEPER_SYNC_MAX_BLOB`, `..._MAX_ROOM_BYTES`, `..._MAX_FILES`, `..._WRITES_PER_MINUTE`, `..._ROOMS_PER_HOUR` |

Container: listens on 8080, data in the `/data` volume, runs as a non-root user, healthcheck `GET /healthz`. Image: `ghcr.io/yonatankarp/lorekeeper-sync`, tags `sha-<commit>` and `latest` from main, built by `.github/workflows/sync-server.yml` when `sync-server/` or `sync-protocol/` change.

## Client behaviour (app)

- A campaign syncs when it's **Shared with my party** and has a room (created from **Create invite link**, or joined from a link). Settings keep `server` and `room` per campaign; `key` and `token` go to the keychain.
- **What syncs:** every file in the campaign folder except hidden files and folders (`.obsidian`, `.DS_Store`, `.trash`), sync conflict copies, and the private folder (which lives outside the campaign folder anyway). Images and portraits sync too.
- **State:** per campaign, a local file in the app's config folder maps `path -> {id, seq, hash}` and stores the room's last `seq`.
- **Pull:** a long-poll loop on `changes`. For each change: decrypt; if the local file is unchanged since it last synced (hash matches state) or absent, write the new content (or delete it); if it also changed locally, keep the local file and save the incoming version beside it as `<name> (conflict <date>).md` (flagged on Home's Sync conflicts card).
- **Push:** the app's own writes and the 3-second folder watch mark changed paths; each is uploaded with `base` = its last seen `seq`. On `409`, apply the pull rule to the returned version.
- **Per-player session files** mean the common case, everyone writing notes during a game, never conflicts.
- Offline or server down: changes queue (they're just files that differ from the state), and sync resumes when the server answers. Status in Settings: "Synced", "Syncing...", "Offline, will sync when the server is back".
