//! Lorekeeper sync protocol, shared by the app and the sync server. docs/SYNC.md is the spec;
//! the known-answer tests below freeze the byte formats it describes.

use std::fmt;
use std::sync::LazyLock;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use data_encoding::{Encoding, Specification, BASE64, BASE64URL_NOPAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Length in bytes of rooms, invites, member ids and file ids.
pub const ID_LEN: usize = 16;
/// Length in bytes of the room key and of tokens.
pub const SECRET_LEN: usize = 32;
pub const NONCE_LEN: usize = 24;
pub const TAG_LEN: usize = 16;
/// Header that carries the server's room creation key, when it has one.
pub const CREATE_KEY_HEADER: &str = "x-lorekeeper-create-key";
/// WebSocket subprotocol the server selects; clients that can't set headers also offer
/// `token.<token>` next to it.
pub const WS_PROTOCOL: &str = "lorekeeper";
pub const WS_TOKEN_PROTOCOL_PREFIX: &str = "token.";

const FILE_ID_CONTEXT: &[u8] = b"lorekeeper file id v1";
const PRIVATE_FILE_ID_CONTEXT: &[u8] = b"lorekeeper private file id v1";

/// Lowercase RFC 4648 base32 without padding.
static BASE32: LazyLock<Encoding> = LazyLock::new(|| {
    let mut spec = Specification::new();
    spec.symbols.push_str("abcdefghijklmnopqrstuvwxyz234567");
    spec.encoding().expect("valid base32 spec")
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Not valid base32/base64 or the wrong length.
    Encoding,
    /// Wrong key, tampered blob, a blob that belongs to another file or room, or one whose path
    /// isn't a safe relative path matching its id.
    Decrypt,
    /// The invite link isn't one Lorekeeper made; the text says what's wrong.
    Invite(&'static str),
    /// Not a sync server address Lorekeeper talks to; the text says what's wrong.
    Server(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Encoding => f.write_str("bad encoding"),
            Error::Decrypt => f.write_str("can't decrypt"),
            Error::Invite(why) => write!(f, "bad invite link: {why}"),
            Error::Server(why) => write!(f, "bad server address: {why}"),
        }
    }
}

impl std::error::Error for Error {}

fn random<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    getrandom::fill(&mut buf).expect("OS random number generator");
    buf
}

/// A new room, invite or member id: 16 random bytes, base32.
pub fn random_id() -> String {
    BASE32.encode(&random::<ID_LEN>())
}

/// A new room key or token: 32 random bytes.
pub fn random_secret() -> [u8; SECRET_LEN] {
    random()
}

/// True for a well-formed room, invite, member or file id.
pub fn is_id(s: &str) -> bool {
    s.len() == 26 && BASE32.decode(s.as_bytes()).is_ok_and(|b| b.len() == ID_LEN)
}

/// Key or token as base64url without padding.
pub fn encode_secret(secret: &[u8; SECRET_LEN]) -> String {
    BASE64URL_NOPAD.encode(secret)
}

pub fn decode_secret(s: &str) -> Result<[u8; SECRET_LEN], Error> {
    let bytes = BASE64URL_NOPAD.decode(s.as_bytes()).map_err(|_| Error::Encoding)?;
    bytes.try_into().map_err(|_| Error::Encoding)
}

/// What the server stores for a token: SHA-256 of its 32 bytes.
pub fn token_hash(token: &[u8; SECRET_LEN]) -> [u8; 32] {
    Sha256::digest(token).into()
}

/// The file's id in its room: keyed BLAKE3 of the context string and the path (`/` separators),
/// first 16 bytes, base32.
pub fn file_id(key: &[u8; SECRET_LEN], path: &str) -> String {
    let mut h = blake3::Hasher::new_keyed(key);
    h.update(FILE_ID_CONTEXT);
    h.update(path.as_bytes());
    BASE32.encode(&h.finalize().as_bytes()[..ID_LEN])
}

/// A private file's id: keyed BLAKE3 of its own context string, the author's member id (26 characters) and the
/// path inside their `Private/` folder, first 16 bytes, base32. The context differs from [`file_id`]'s at byte 12,
/// so no private id is ever a shared one, and the member id makes two players' identical private paths different
/// ids, so the server can link them neither to each other nor to a shared page.
pub fn private_file_id(key: &[u8; SECRET_LEN], member_id: &str, path: &str) -> String {
    let mut h = blake3::Hasher::new_keyed(key);
    h.update(PRIVATE_FILE_ID_CONTEXT);
    h.update(member_id.as_bytes());
    h.update(path.as_bytes());
    BASE32.encode(&h.finalize().as_bytes()[..ID_LEN])
}

/// Change detection for clients: BLAKE3 of the plain bytes, lowercase hex.
pub fn content_hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// A blob as it travels in JSON: standard base64 with padding.
pub fn encode_blob(blob: &[u8]) -> String {
    BASE64.encode(blob)
}

pub fn decode_blob(s: &str) -> Result<Vec<u8>, Error> {
    BASE64.decode(s.as_bytes()).map_err(|_| Error::Encoding)
}

/// The plaintext inside a blob.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FileContent {
    pub path: String,
    #[serde(with = "base64_bytes")]
    pub content: Vec<u8>,
    /// Unix milliseconds.
    pub modified: i64,
    /// An encrypted tombstone: the file was deleted (`content` is empty). Clients delete this way, with a normal
    /// `put`, because the server's own delete (a null blob) carries nothing sealed with the key, so anyone holding a
    /// token could send one. Left out of the JSON when false, so ordinary blobs keep their layout.
    #[serde(default, skip_serializing_if = "is_false")]
    pub deleted: bool,
}

fn is_false(b: &bool) -> bool {
    !b
}

impl FileContent {
    /// A tombstone for `path`.
    pub fn tombstone(path: &str, modified: i64) -> FileContent {
        FileContent { path: path.into(), content: Vec::new(), modified, deleted: true }
    }
}

mod base64_bytes {
    use serde::{de::Error, Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::BASE64.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        super::BASE64.decode(s.as_bytes()).map_err(D::Error::custom)
    }
}

/// Associated data: the room and id strings (26 ASCII characters each), concatenated.
fn associated_data(room: &str, id: &str) -> Vec<u8> {
    [room.as_bytes(), id.as_bytes()].concat()
}

fn seal_with_nonce(key: &[u8; SECRET_LEN], nonce: &[u8; NONCE_LEN], aad: &[u8], msg: &[u8]) -> Vec<u8> {
    let cipher = XChaCha20Poly1305::new(&(*key).into());
    let ct = cipher
        .encrypt(&XNonce::from(*nonce), Payload { msg, aad })
        .expect("message within XChaCha20-Poly1305 limits");
    [nonce.as_slice(), &ct].concat()
}

/// Encrypts a file for `id` in `room`: `nonce || ciphertext` (the tag is the last 16 bytes).
pub fn seal(key: &[u8; SECRET_LEN], room: &str, id: &str, file: &FileContent) -> Vec<u8> {
    let json = serde_json::to_vec(file).expect("FileContent serializes");
    seal_with_nonce(key, &random(), &associated_data(room, id), &json)
}

/// True for a relative `/`-separated path that stays inside the campaign folder: no empty, `.`
/// or `..` components, no leading `/`, no `\`, NUL or drive prefix (`C:`).
pub fn is_safe_path(path: &str) -> bool {
    let drive = path.as_bytes().get(1) == Some(&b':') && path.as_bytes()[0].is_ascii_alphabetic();
    !drive && !path.contains(['\\', '\0']) && path.split('/').all(|c| !matches!(c, "" | "." | ".."))
}

fn open_raw(key: &[u8; SECRET_LEN], aad: &[u8], blob: &[u8]) -> Result<Vec<u8>, Error> {
    if blob.len() < NONCE_LEN + TAG_LEN {
        return Err(Error::Decrypt);
    }
    let (nonce, ct) = blob.split_at(NONCE_LEN);
    let nonce: [u8; NONCE_LEN] = nonce.try_into().expect("split at NONCE_LEN");
    let cipher = XChaCha20Poly1305::new(&(*key).into());
    cipher.decrypt(&XNonce::from(nonce), Payload { msg: ct, aad }).map_err(|_| Error::Decrypt)
}

/// Decrypts a blob made by [`seal`] for the same key, room and id. Anyone with the key (any party
/// member) can seal, so the path inside is checked too: it must be safe ([`is_safe_path`]) and be
/// the one `id` was derived from.
pub fn open(key: &[u8; SECRET_LEN], room: &str, id: &str, blob: &[u8]) -> Result<FileContent, Error> {
    let json = open_raw(key, &associated_data(room, id), blob)?;
    let file: FileContent = serde_json::from_slice(&json).map_err(|_| Error::Decrypt)?;
    if !is_safe_path(&file.path) || file_id(key, &file.path) != id {
        return Err(Error::Decrypt);
    }
    Ok(file)
}

/// [`open`] for a file in `member_id`'s private space: the path inside (relative to their `Private/` folder) must be
/// safe and be the one `id` was derived from with [`private_file_id`] for that member. A shared blob, or one moved to
/// another member's space, doesn't open.
pub fn open_private(key: &[u8; SECRET_LEN], room: &str, member_id: &str, id: &str, blob: &[u8]) -> Result<FileContent, Error> {
    let json = open_raw(key, &associated_data(room, id), blob)?;
    let file: FileContent = serde_json::from_slice(&json).map_err(|_| Error::Decrypt)?;
    if !is_id(member_id) || !is_safe_path(&file.path) || private_file_id(key, member_id, &file.path) != id {
        return Err(Error::Decrypt);
    }
    Ok(file)
}

const MEMBER_CONTEXT: &[u8] = b"lorekeeper member name v1";
/// Longest name [`seal_member`] keeps (bytes); the sealed display id then fits the server's 512.
pub const MAX_MEMBER_NAME: usize = 300;

fn member_aad(room: &str) -> Vec<u8> {
    [room.as_bytes(), MEMBER_CONTEXT].concat()
}

/// A member display id: the player's name sealed with the room key (`nonce || ciphertext || tag`,
/// standard base64), so only the party can read it. An empty name stays "". Names are cut to
/// [`MAX_MEMBER_NAME`] bytes.
pub fn seal_member(key: &[u8; SECRET_LEN], room: &str, name: &str) -> String {
    let mut end = name.len().min(MAX_MEMBER_NAME);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    if end == 0 {
        return String::new();
    }
    BASE64.encode(&seal_with_nonce(key, &random(), &member_aad(room), &name.as_bytes()[..end]))
}

/// The name in a display id made by [`seal_member`] for the same key and room ("" for "").
pub fn open_member(key: &[u8; SECRET_LEN], room: &str, member: &str) -> Result<String, Error> {
    if member.is_empty() {
        return Ok(String::new());
    }
    let blob = BASE64.decode(member.as_bytes()).map_err(|_| Error::Encoding)?;
    String::from_utf8(open_raw(key, &member_aad(room), &blob)?).map_err(|_| Error::Decrypt)
}

/// A sync server address as `https://host[:port]` (plain `http://` only for localhost,
/// 127.0.0.1 and [::1]), the same rule as invite links. Surrounding whitespace and one trailing
/// `/` are ignored and the host is lowercased; anything else after the host is refused.
pub fn server_origin(url: &str) -> Result<String, Error> {
    let url = url.trim();
    let url = url.strip_suffix('/').unwrap_or(url);
    let (scheme, authority) = if let Some(rest) = url.strip_prefix("https://") {
        ("https", rest)
    } else if let Some(rest) = url.strip_prefix("http://") {
        ("http", rest)
    } else {
        return Err(Error::Server("not an https address"));
    };
    if authority.contains(['/', '?', '#', '@']) {
        return Err(Error::Server("only the server's address, without a path"));
    }
    let authority = authority.to_ascii_lowercase();
    let host = parse_authority(&authority).map_err(|_| Error::Server("bad host or port"))?;
    if scheme == "http" && !matches!(host, "localhost" | "127.0.0.1" | "[::1]") {
        return Err(Error::Server("http is only allowed for localhost"));
    }
    Ok(format!("{scheme}://{authority}"))
}

/// An invite link: `<server>/join/<room>/<invite>#<key>`. The key stays in the fragment, which
/// browsers and HTTP clients never send to the server.
#[derive(Clone, PartialEq, Eq)]
pub struct Invite {
    /// `https://host[:port]` (or `http://` for localhost), the server to talk to.
    pub server: String,
    pub room: String,
    pub invite: String,
    pub key: [u8; SECRET_LEN],
}

impl fmt::Debug for Invite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Invite")
            .field("server", &self.server)
            .field("room", &self.room)
            .field("invite", &"<redacted>")
            .field("key", &"<redacted>")
            .finish()
    }
}

impl Invite {
    pub fn link(&self) -> String {
        format!("{}/join/{}/{}#{}", self.server, self.room, self.invite, encode_secret(&self.key))
    }

    /// Parses a pasted link strictly; surrounding whitespace is ignored and the host is
    /// lowercased.
    pub fn parse(link: &str) -> Result<Invite, Error> {
        let link = link.trim();
        let (before, key) = link.split_once('#').ok_or(Error::Invite("no key after #"))?;
        let key = decode_secret(key).map_err(|_| Error::Invite("the key is damaged"))?;
        let (scheme, rest) = if let Some(rest) = before.strip_prefix("https://") {
            ("https", rest)
        } else if let Some(rest) = before.strip_prefix("http://") {
            ("http", rest)
        } else {
            return Err(Error::Invite("not an https link"));
        };
        let (authority, path) = rest.split_once('/').ok_or(Error::Invite("no /join/ path"))?;
        let authority = authority.to_ascii_lowercase();
        let host = parse_authority(&authority)?;
        if scheme == "http" && !matches!(host, "localhost" | "127.0.0.1" | "[::1]") {
            return Err(Error::Invite("http is only allowed for localhost"));
        }
        let mut parts = path.split('/');
        let (Some("join"), Some(room), Some(invite), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(Error::Invite("the path isn't /join/<room>/<invite>"));
        };
        if !is_id(room) {
            return Err(Error::Invite("the room is damaged"));
        }
        if !is_id(invite) {
            return Err(Error::Invite("the invite is damaged"));
        }
        Ok(Invite {
            server: format!("{scheme}://{authority}"),
            room: room.to_string(),
            invite: invite.to_string(),
            key,
        })
    }
}

/// Validates `host[:port]` (already lowercase) and returns the host.
fn parse_authority(authority: &str) -> Result<&str, Error> {
    let (host, port) = if authority.starts_with('[') {
        let end = authority.find(']').ok_or(Error::Invite("bad host"))?;
        let (host, rest) = authority.split_at(end + 1);
        let inner = &host[1..host.len() - 1];
        if inner.is_empty() || !inner.chars().all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.') {
            return Err(Error::Invite("bad host"));
        }
        match rest {
            "" => (host, None),
            _ => (host, Some(rest.strip_prefix(':').ok_or(Error::Invite("bad host"))?)),
        }
    } else {
        match authority.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (authority, None),
        }
    };
    let host_ok = host.starts_with('[')
        || (!host.is_empty()
            && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
            && !host.starts_with(['.', '-'])
            && !host.ends_with(['.', '-']));
    if !host_ok {
        return Err(Error::Invite("bad host"));
    }
    if let Some(port) = port {
        let ok = !port.is_empty()
            && port.len() <= 5
            && port.bytes().all(|b| b.is_ascii_digit())
            && port.parse::<u16>().is_ok_and(|p| p != 0);
        if !ok {
            return Err(Error::Invite("bad port"));
        }
    }
    Ok(host)
}

// ---- HTTP API (v1) ----

/// A member's role. Exactly one owner (the room's creator); `dm` and `player` come from the invite they redeemed and
/// can be changed by the owner. Rooms from before roles had `member`, which reads as `player`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    Dm,
    #[default]
    #[serde(alias = "member")]
    Player,
}

impl Role {
    /// Whether this member receives other members' private notes: only while the room's `dm_reads_private` is on, and
    /// only a DM, or the owner when they said they're also the DM (`owner_is_dm`, theirs alone to set). Being the owner
    /// alone reads nothing; players never do. The server's delivery and the app's labels both follow this one rule.
    pub fn reads_private(self, owner_is_dm: bool, dm_reads_private: bool) -> bool {
        dm_reads_private && (self == Role::Dm || (self == Role::Owner && owner_is_dm))
    }
}

/// Where a write goes: the room's shared files, or the writer's own private space (`Private/` in their campaign
/// folder), which the server delivers only to that member's devices and, when the room allows, to DMs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Space {
    #[default]
    Shared,
    Private,
}

impl Space {
    fn is_shared(&self) -> bool {
        *self == Space::Shared
    }
}

/// `POST /v1/rooms` answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateRoomResponse {
    pub room: String,
    pub owner_token: String,
}

/// `POST /v1/rooms/{room}/invites` body (optional: none means a player).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct InviteRequest {
    #[serde(default)]
    pub role: Role,
    /// Only for a DM, and only the owner may grant it: the DM can then invite players and DMs, list and cancel
    /// invites, and remove or re-invite members (never the owner or another manager).
    #[serde(default)]
    pub manage: bool,
}

/// `POST /v1/rooms/{room}/invites` and `.../members/{member_id}/reinvite` answer, and an item of
/// `GET /v1/rooms/{room}/invites`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InviteInfo {
    pub invite: String,
    /// Unix milliseconds.
    pub expires: i64,
    #[serde(default)]
    pub role: Role,
    #[serde(default)]
    pub manage: bool,
    /// A re-invite: the member whose identity (role and private notes) redeeming it hands over.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_id: Option<String>,
}

/// `PATCH /v1/rooms/{room}/members/{member_id}` body (owner only); a missing field stays as it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MemberUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manage: Option<bool>,
    /// Only on the owner's own row: "I'm also the DM".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_is_dm: Option<bool>,
}

/// `PATCH /v1/rooms/{room}/settings` body and answer (owner only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RoomSettings {
    /// DMs (and the owner) receive every member's private notes; off by default.
    pub dm_reads_private: bool,
}

/// `POST /v1/rooms/{room}/invites/{invite}/redeem` body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedeemRequest {
    /// Opaque display id; clients encrypt it with the room key.
    pub member: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedeemResponse {
    pub token: String,
}

/// An item of `GET /v1/rooms/{room}/members`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberInfo {
    pub member_id: String,
    pub member: String,
    pub role: Role,
    #[serde(default)]
    pub manage: bool,
    /// The owner said they're also the campaign's DM (see [`Role::reads_private`]); false for everyone else.
    #[serde(default)]
    pub owner_is_dm: bool,
    /// Unix milliseconds.
    pub created: i64,
    pub last_seen: Option<i64>,
}

/// One file's latest version; `blob` is `None` for a deleted file (tombstone). `author` is set for a private file:
/// the member whose private space it is in (open it with [`open_private`] for that member).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub id: String,
    pub seq: u64,
    pub blob: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
}

/// `GET /v1/rooms/{room}/changes?since=N` answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangesResponse {
    pub seq: u64,
    pub changes: Vec<Change>,
    pub more: bool,
}

/// Body of every 4xx/5xx answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
}

// ---- WebSocket (`GET /v1/rooms/{room}/live`), JSON text frames ----

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ClientMessage {
    /// First message: replays changes after `since`, then streams live. Other members' private files (sent only to
    /// those who read them) replay after `dm_since` instead when it's given, so a DM who just gained access gets
    /// all of them.
    Hello {
        since: u64,
        member: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dm_since: Option<u64>,
    },
    /// A write into the shared files or the writer's own private space; nothing can name anyone else's.
    Put {
        req: u64,
        id: String,
        base: u64,
        blob: String,
        #[serde(default, skip_serializing_if = "Space::is_shared")]
        space: Space,
    },
    Delete {
        req: u64,
        id: String,
        base: u64,
        #[serde(default, skip_serializing_if = "Space::is_shared")]
        space: Space,
    },
    Ping,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceMember {
    pub member_id: String,
    pub member: String,
    pub role: Role,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ServerMessage {
    /// A replay batch after `hello`; the last one has `more: false`. `total`: how many changes the whole replay holds
    /// (counted when it starts; live writes meanwhile can add a few), for a progress bar. Servers before it leave it out.
    Changes {
        seq: u64,
        changes: Vec<Change>,
        more: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        total: Option<u64>,
    },
    /// A live write by another connection.
    Change(Change),
    Presence { members: Vec<PresenceMember> },
    Ack { req: u64, seq: u64 },
    Conflict { req: u64, seq: u64, blob: Option<String> },
    /// Who this connection is and what it may read: after `hello` (before the replay) and whenever any of it changes.
    /// `members` lists the room's members (ids, display ids, roles) only when this member reads others' private
    /// notes (see [`Role::reads_private`]), so the app can drop its copies of anyone no longer there.
    Access {
        member_id: String,
        role: Role,
        manage: bool,
        /// The owner said they're also the DM (always false for anyone else).
        #[serde(default)]
        owner_is_dm: bool,
        dm_reads_private: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        members: Vec<PresenceMember>,
    },
    Pong,
    Error {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        req: Option<u64>,
        error: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; 32] = [7u8; 32];

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    fn file() -> FileContent {
        FileContent { path: "Sessions/Session 4/Sibling 5.md".into(), content: b"# Notes\n".to_vec(), modified: 1_700_000_000_000, ..Default::default() }
    }

    #[test]
    fn base32_matches_rfc4648_lowercased() {
        for (plain, enc) in [("", ""), ("f", "my"), ("fo", "mzxq"), ("foo", "mzxw6"), ("foobar", "mzxw6ytboi")] {
            assert_eq!(BASE32.encode(plain.as_bytes()), enc);
        }
    }

    #[test]
    fn ids_are_26_lowercase_chars_and_validate_strictly() {
        let id = random_id();
        assert_eq!(id.len(), 26);
        assert!(is_id(&id));
        assert_ne!(id, random_id());
        assert!(!is_id(&id.to_uppercase()));
        assert!(!is_id(&id[..25]));
        assert!(!is_id(&format!("{id}a")));
        assert!(!is_id("aaaaaaaaaaaaaaaaaaaaaaaaa1")); // 1 isn't a base32 symbol
        // The last character carries 2 unused bits; non-zero ones aren't a canonical encoding.
        assert!(is_id("aaaaaaaaaaaaaaaaaaaaaaaaaa"));
        assert!(is_id("aaaaaaaaaaaaaaaaaaaaaaaaae"));
        assert!(!is_id("aaaaaaaaaaaaaaaaaaaaaaaaab"));
    }

    #[test]
    fn secrets_round_trip_as_base64url() {
        let s = random_secret();
        let enc = encode_secret(&s);
        assert_eq!(enc.len(), 43);
        assert!(!enc.contains(['+', '/', '=']));
        assert_eq!(decode_secret(&enc), Ok(s));
        assert_eq!(encode_secret(&[0xff; 32]), "_".repeat(42) + "8");
        assert_eq!(decode_secret(&enc[..42]), Err(Error::Encoding));
        assert_eq!(decode_secret(&format!("{enc}=")), Err(Error::Encoding));
        assert_eq!(decode_secret(&"+".repeat(43)), Err(Error::Encoding), "standard base64 alphabet");
        assert_eq!(decode_secret(&("A".repeat(42) + "B")), Err(Error::Encoding), "non-zero trailing bits");
    }

    #[test]
    fn token_hash_is_sha256() {
        assert_eq!(token_hash(&[0u8; 32]).to_vec(), hex("66687aadf862bd776c8fc18b8e9f8e20089714856ee233b3902a591d0d5f2925"));
    }

    #[test]
    fn content_hash_is_blake3_hex() {
        assert_eq!(content_hash(b""), "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262");
    }

    #[test]
    fn file_id_known_answer() {
        // Frozen: changing this breaks every synced campaign.
        assert_eq!(file_id(&KEY, "Sessions/Session 4/Sibling 5.md"), FILE_ID_KAT);
        assert_ne!(file_id(&KEY, "Sessions/Session 4/Sibling 6.md"), FILE_ID_KAT);
        assert_ne!(file_id(&[8u8; 32], "Sessions/Session 4/Sibling 5.md"), FILE_ID_KAT);
        assert!(is_id(FILE_ID_KAT));
    }
    const FILE_ID_KAT: &str = "p2mx5mbyllx2azntdkyzsvfoi4";

    #[test]
    fn xchacha20poly1305_matches_the_draft_test_vector() {
        // draft-irtf-cfrg-xchacha-03, A.3.1
        let key: [u8; 32] = hex("808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f").try_into().unwrap();
        let nonce: [u8; 24] = hex("404142434445464748494a4b4c4d4e4f5051525354555657").try_into().unwrap();
        let aad = hex("50515253c0c1c2c3c4c5c6c7");
        let msg = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
        let out = seal_with_nonce(&key, &nonce, &aad, msg);
        let expected = hex(concat!(
            "bd6d179d3e83d43b9576579493c0e939572a1700252bfaccbed2902c21396cbb",
            "731c7f1b0b4aa6440bf3a82f4eda7e39ae64c6708c54c216cb96b72e1213b452",
            "2f8c9ba40db5d945b11b69b982c1bb9e3f3fac2bc369488f76b2383565d3fff9",
            "21f9664c97637da9768812f615c68b13b52e",
            "c0875924c1c7987947deafd8780acf49"
        ));
        assert_eq!(&out[..24], &nonce);
        assert_eq!(out[24..].to_vec(), expected);
    }

    #[test]
    fn blob_known_answer() {
        let room = "aaaaaaaaaaaaaaaaaaaaaaaaaa";
        let id = file_id(&KEY, &file().path);
        let json = serde_json::to_vec(&file()).unwrap();
        assert_eq!(json, br#"{"path":"Sessions/Session 4/Sibling 5.md","content":"IyBOb3Rlcwo=","modified":1700000000000}"#);
        let blob = seal_with_nonce(&KEY, &[1u8; 24], &associated_data(room, &id), &json);
        assert_eq!(encode_blob(&blob), BLOB_KAT);
        assert_eq!(open(&KEY, room, &id, &blob), Ok(file()));
    }
    const BLOB_KAT: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBylZzLL2Ez6UfuPLwVGI448RowUDXB2LA3z+LGjOAGyMWgChUuaMgJVDtSLbz4JVFkVTSvzIExlli7U2QJlUprcbwksOyL1FpQCfNpGxgEzUHxd1cp8XtKTds8yPe9I2NPhOV7wtg4Nc8reKC";

    #[test]
    fn blob_round_trips_with_fresh_nonces() {
        let (room, id) = (random_id(), file_id(&KEY, &file().path));
        let a = seal(&KEY, &room, &id, &file());
        let b = seal(&KEY, &room, &id, &file());
        assert_ne!(a, b);
        assert_eq!(a.len(), NONCE_LEN + serde_json::to_vec(&file()).unwrap().len() + TAG_LEN);
        assert_eq!(open(&KEY, &room, &id, &a), Ok(file()));
        assert_eq!(open(&KEY, &room, &id, &decode_blob(&encode_blob(&b)).unwrap()), Ok(file()));
        let empty = FileContent { path: "a.md".into(), ..Default::default() };
        let id = file_id(&KEY, "a.md");
        assert_eq!(open(&KEY, &room, &id, &seal(&KEY, &room, &id, &empty)), Ok(empty));
    }

    #[test]
    fn open_rejects_unsafe_or_mismatched_paths() {
        // A party member holds the key, so they can seal any path; open() must not trust it.
        let room = random_id();
        for path in ["../x.md", "a/../../x.md", "/etc/passwd", "a//b.md", "./a.md", "a/.", "", "a\\..\\b.md", "C:x.md", "c:/x.md", "a\0.md"] {
            assert!(!is_safe_path(path), "{path:?}");
            let id = file_id(&KEY, path);
            let f = FileContent { path: path.into(), ..Default::default() };
            assert_eq!(open(&KEY, &room, &id, &seal(&KEY, &room, &id, &f)), Err(Error::Decrypt), "{path:?}");
        }
        for path in ["a.md", "Sessions/Session 4/Sibling 5.md", ".obsidian/x", "a..b.md", "Session 1: Start.md"] {
            assert!(is_safe_path(path), "{path:?}");
        }
        // A safe path under another file's id: the id must come from the path.
        let other = file_id(&KEY, "b.md");
        let f = FileContent { path: "a.md".into(), ..Default::default() };
        assert_eq!(open(&KEY, &room, &other, &seal(&KEY, &room, &other, &f)), Err(Error::Decrypt));
    }

    #[test]
    fn tampering_is_detected() {
        let (room, id) = (random_id(), random_id());
        let blob = seal(&KEY, &room, &id, &file());
        assert_eq!(open(&[8u8; 32], &room, &id, &blob), Err(Error::Decrypt), "wrong key");
        assert_eq!(open(&KEY, &room, &random_id(), &blob), Err(Error::Decrypt), "moved to another file");
        assert_eq!(open(&KEY, &random_id(), &id, &blob), Err(Error::Decrypt), "moved to another room");
        assert_eq!(open(&KEY, &id, &room, &blob), Err(Error::Decrypt), "room and id swapped");
        for i in [0, 23, 24, blob.len() / 2, blob.len() - 1] {
            let mut bad = blob.clone();
            bad[i] ^= 0x01;
            assert_eq!(open(&KEY, &room, &id, &bad), Err(Error::Decrypt), "bit flip at {i}");
        }
        assert_eq!(open(&KEY, &room, &id, &blob[..blob.len() - 1]), Err(Error::Decrypt), "truncated");
        assert_eq!(open(&KEY, &room, &id, &blob[..39]), Err(Error::Decrypt), "shorter than nonce and tag");
        assert_eq!(open(&KEY, &room, &id, &[]), Err(Error::Decrypt));
    }

    #[test]
    fn non_json_plaintext_is_rejected() {
        let (room, id) = (random_id(), random_id());
        let blob = seal_with_nonce(&KEY, &[0u8; 24], &associated_data(&room, &id), b"not json");
        assert_eq!(open(&KEY, &room, &id, &blob), Err(Error::Decrypt));
    }

    fn invite() -> Invite {
        Invite { server: "https://lorekeeper.yonatankarp.com".into(), room: random_id(), invite: random_id(), key: random_secret() }
    }

    #[test]
    fn invite_link_round_trips() {
        let inv = invite();
        let link = inv.link();
        assert!(link.starts_with(&format!("https://lorekeeper.yonatankarp.com/join/{}/{}#", inv.room, inv.invite)));
        assert_eq!(Invite::parse(&link), Ok(inv.clone()));
        assert_eq!(Invite::parse(&format!("  {link}\n")), Ok(inv.clone()));
        let upper = link.replace("lorekeeper.yonatankarp.com", "Lorekeeper.YonatanKarp.com");
        assert_eq!(Invite::parse(&upper), Ok(inv.clone()));
        for server in ["https://sync.example.org:8443", "http://localhost:8080", "http://127.0.0.1:8080", "http://[::1]:8080", "https://[2001:db8::1]"] {
            let other = Invite { server: server.into(), ..inv.clone() };
            assert_eq!(Invite::parse(&other.link()), Ok(other), "{server}");
        }
    }

    #[test]
    fn invite_debug_hides_secrets() {
        let inv = invite();
        let debug = format!("{inv:?}");
        assert!(!debug.contains(&encode_secret(&inv.key)));
        assert!(!debug.contains(&inv.invite));
    }

    #[test]
    fn invite_parsing_rejects_anything_odd() {
        let inv = invite();
        let (r, i, k) = (inv.room.clone(), inv.invite.clone(), encode_secret(&inv.key));
        let bad = [
            String::new(),
            format!("https://h.com/join/{r}/{i}"),
            format!("https://h.com/join/{r}/{i}#"),
            format!("https://h.com/join/{r}/{i}#{}", &k[1..]),
            format!("https://h.com/join/{r}/{i}#{k}.{k}"),
            format!("https://h.com/join/{r}/{i}#{k}#"),
            format!("ftp://h.com/join/{r}/{i}#{k}"),
            format!("HTTPS://h.com/join/{r}/{i}#{k}"),
            format!("http://h.com/join/{r}/{i}#{k}"),
            format!("http://192.168.1.2/join/{r}/{i}#{k}"),
            format!("h.com/join/{r}/{i}#{k}"),
            format!("https://h.com#{k}"),
            format!("https:///join/{r}/{i}#{k}"),
            format!("https://user@h.com/join/{r}/{i}#{k}"),
            format!("https://h.com:/join/{r}/{i}#{k}"),
            format!("https://h.com:0/join/{r}/{i}#{k}"),
            format!("https://h.com:65536/join/{r}/{i}#{k}"),
            format!("https://h.com:80a/join/{r}/{i}#{k}"),
            format!("https://h_x.com/join/{r}/{i}#{k}"),
            format!("https://-h.com/join/{r}/{i}#{k}"),
            format!("https://[::1/join/{r}/{i}#{k}"),
            format!("https://[]/join/{r}/{i}#{k}"),
            format!("https://[::1]x/join/{r}/{i}#{k}"),
            format!("https://h.com/join/{r}#{k}"),
            format!("https://h.com/join/{r}/{i}/#{k}"),
            format!("https://h.com/join/{r}/{i}?x=1#{k}"),
            format!("https://h.com/sub/join/{r}/{i}#{k}"),
            format!("https://h.com/Join/{r}/{i}#{k}"),
            format!("https://h.com/join/{}/{i}#{k}", r.to_uppercase()),
            format!("https://h.com/join/{r}/{}#{k}", &i[..25]),
            format!("https://h.com/join/{r}/{i}#{}", k.replace(|c: char| c.is_ascii_alphanumeric(), "+")),
            format!("https://h.com/join/{r}/{i} x#{k}"),
            format!("https://h .com/join/{r}/{i}#{k}"),
        ];
        for link in bad {
            assert!(matches!(Invite::parse(&link), Err(Error::Invite(_))), "accepted {link:?}");
        }
    }

    #[test]
    fn tombstones_are_sealed_like_files() {
        let room = random_id();
        let id = file_id(&KEY, "NPCs/Vex.md");
        let tomb = FileContent::tombstone("NPCs/Vex.md", 5);
        assert_eq!(serde_json::to_string(&tomb).unwrap(), r#"{"path":"NPCs/Vex.md","content":"","modified":5,"deleted":true}"#);
        assert_eq!(open(&KEY, &room, &id, &seal(&KEY, &room, &id, &tomb)), Ok(tomb.clone()));
        // An ordinary file has no "deleted" field, and older blobs without one open as files.
        assert!(!serde_json::to_string(&file()).unwrap().contains("deleted"));
        let old: FileContent = serde_json::from_str(r#"{"path":"a.md","content":"","modified":0}"#).unwrap();
        assert!(!old.deleted);
        // Only someone with the key can make one: a tombstone sealed with another key doesn't open.
        assert_eq!(open(&KEY, &room, &id, &seal(&[8u8; 32], &room, &id, &tomb)), Err(Error::Decrypt));
    }

    #[test]
    fn member_names_round_trip_and_stay_private() {
        let room = random_id();
        let sealed = seal_member(&KEY, &room, "Lorelei");
        assert!(!sealed.contains("Lorelei"));
        assert_ne!(sealed, seal_member(&KEY, &room, "Lorelei"), "fresh nonce each time");
        assert_eq!(open_member(&KEY, &room, &sealed), Ok("Lorelei".into()));
        assert_eq!(open_member(&[8u8; 32], &room, &sealed), Err(Error::Decrypt), "wrong key");
        assert_eq!(open_member(&KEY, &random_id(), &sealed), Err(Error::Decrypt), "another room");
        assert_eq!(seal_member(&KEY, &room, ""), "");
        assert_eq!(open_member(&KEY, &room, ""), Ok(String::new()));
        assert_eq!(open_member(&KEY, &room, "not base64!"), Err(Error::Encoding));
        assert_eq!(open_member(&KEY, &room, "AAAA"), Err(Error::Decrypt), "too short");
        // A file blob can't pass for a member name, nor the other way round.
        let id = file_id(&KEY, "a.md");
        let blob = encode_blob(&seal(&KEY, &room, &id, &file()));
        assert_eq!(open_member(&KEY, &room, &blob), Err(Error::Decrypt));
        assert_eq!(open(&KEY, &room, &id, &decode_blob(&sealed).unwrap()), Err(Error::Decrypt));
        // Long names are cut on a character boundary and still fit the server's 512 bytes.
        let long = "é".repeat(400);
        let sealed = seal_member(&KEY, &room, &long);
        assert!(sealed.len() <= 512, "{}", sealed.len());
        assert_eq!(open_member(&KEY, &room, &sealed).unwrap(), "é".repeat(MAX_MEMBER_NAME / 2));
    }

    #[test]
    fn server_origins_are_strict() {
        for (url, origin) in [
            ("https://lorekeeper.yonatankarp.com", "https://lorekeeper.yonatankarp.com"),
            (" https://Sync.Example.org:8443/ ", "https://sync.example.org:8443"),
            ("http://127.0.0.1:18081", "http://127.0.0.1:18081"),
            ("http://localhost:8080/", "http://localhost:8080"),
            ("http://[::1]:8080", "http://[::1]:8080"),
        ] {
            assert_eq!(server_origin(url), Ok(origin.into()), "{url}");
        }
        for url in [
            "", "lorekeeper.yonatankarp.com", "ftp://h.com", "http://h.com", "http://192.168.1.2:8080", "https://h.com/sub",
            "https://h.com?x=1", "https://h.com#k", "https://user@h.com", "https://h.com:0", "https://h_x.com", "https://",
            "https://h.com//", "wss://h.com", "http://localhost.evil.com",
        ] {
            assert!(matches!(server_origin(url), Err(Error::Server(_))), "accepted {url:?}");
        }
    }

    #[test]
    fn wire_types_match_the_doc() {
        let json = |v: &ServerMessage| serde_json::to_string(v).unwrap();
        let change = Change { id: "i".into(), seq: 3, blob: None, author: None };
        assert_eq!(json(&ServerMessage::Change(change.clone())), r#"{"type":"change","id":"i","seq":3,"blob":null}"#);
        let private = Change { author: Some("m".into()), ..change.clone() };
        assert_eq!(json(&ServerMessage::Change(private)), r#"{"type":"change","id":"i","seq":3,"blob":null,"author":"m"}"#);
        assert_eq!(json(&ServerMessage::Ack { req: 1, seq: 2 }), r#"{"type":"ack","req":1,"seq":2}"#);
        assert_eq!(json(&ServerMessage::Pong), r#"{"type":"pong"}"#);
        assert_eq!(json(&ServerMessage::Error { req: None, error: "x".into() }), r#"{"type":"error","error":"x"}"#);
        assert_eq!(json(&ServerMessage::Error { req: Some(4), error: "x".into() }), r#"{"type":"error","req":4,"error":"x"}"#);
        assert_eq!(
            json(&ServerMessage::Presence { members: vec![PresenceMember { member_id: "m".into(), member: "n".into(), role: Role::Owner }] }),
            r#"{"type":"presence","members":[{"member_id":"m","member":"n","role":"owner"}]}"#
        );
        assert_eq!(
            json(&ServerMessage::Changes { seq: 3, changes: vec![change.clone()], more: false, total: None }),
            r#"{"type":"changes","seq":3,"changes":[{"id":"i","seq":3,"blob":null}],"more":false}"#
        );
        assert_eq!(
            json(&ServerMessage::Changes { seq: 3, changes: vec![change], more: false, total: Some(1) }),
            r#"{"type":"changes","seq":3,"changes":[{"id":"i","seq":3,"blob":null}],"more":false,"total":1}"#
        );
        let access = ServerMessage::Access { member_id: "m".into(), role: Role::Dm, manage: true, owner_is_dm: false, dm_reads_private: false, members: vec![] };
        assert_eq!(json(&access), r#"{"type":"access","member_id":"m","role":"dm","manage":true,"owner_is_dm":false,"dm_reads_private":false}"#);
        let parse = |s: &str| serde_json::from_str::<ClientMessage>(s).unwrap();
        assert_eq!(parse(r#"{"type":"hello","since":0,"member":"x"}"#), ClientMessage::Hello { since: 0, member: "x".into(), dm_since: None });
        assert_eq!(parse(r#"{"type":"hello","since":5,"member":"","dm_since":0}"#), ClientMessage::Hello { since: 5, member: "".into(), dm_since: Some(0) });
        assert_eq!(parse(r#"{"type":"ping"}"#), ClientMessage::Ping);
        let put = ClientMessage::Put { req: 1, id: "i".into(), base: 0, blob: "AA==".into(), space: Space::Shared };
        assert_eq!(parse(r#"{"type":"put","req":1,"id":"i","base":0,"blob":"AA=="}"#), put);
        assert_eq!(serde_json::to_string(&put).unwrap(), r#"{"type":"put","req":1,"id":"i","base":0,"blob":"AA=="}"#, "shared is left out");
        assert_eq!(
            parse(r#"{"type":"put","req":1,"id":"i","base":0,"blob":"AA==","space":"private"}"#),
            ClientMessage::Put { req: 1, id: "i".into(), base: 0, blob: "AA==".into(), space: Space::Private }
        );
        assert_eq!(parse(r#"{"type":"delete","req":1,"id":"i","base":2}"#), ClientMessage::Delete { req: 1, id: "i".into(), base: 2, space: Space::Shared });
        assert!(serde_json::from_str::<ClientMessage>(r#"{"type":"put","req":1,"id":"i","base":0,"blob":"AA==","space":"theirs"}"#).is_err());
        assert!(serde_json::from_str::<ClientMessage>(r#"{"type":"shout"}"#).is_err());
        // Roles: rooms from before roles said "member", which is a player now.
        assert_eq!(serde_json::to_string(&Role::Player).unwrap(), r#""player""#);
        assert_eq!(serde_json::from_str::<Role>(r#""member""#).unwrap(), Role::Player);
        assert_eq!(serde_json::from_str::<Role>(r#""dm""#).unwrap(), Role::Dm);
        assert_eq!(serde_json::from_str::<InviteRequest>("{}").unwrap(), InviteRequest { role: Role::Player, manage: false });
        let old: InviteInfo = serde_json::from_str(r#"{"invite":"i","expires":1}"#).unwrap();
        assert_eq!((old.role, old.manage, old.member_id), (Role::Player, false, None));
        assert_eq!(serde_json::to_string(&MemberUpdate { role: Some(Role::Dm), ..Default::default() }).unwrap(), r#"{"role":"dm"}"#);
        assert_eq!(serde_json::from_str::<MemberUpdate>(r#"{"owner_is_dm":true}"#).unwrap().owner_is_dm, Some(true));
        assert_eq!(serde_json::to_string(&RoomSettings { dm_reads_private: true }).unwrap(), r#"{"dm_reads_private":true}"#);
    }

    #[test]
    fn only_dms_read_private_notes_and_only_when_allowed() {
        for role in [Role::Owner, Role::Dm, Role::Player] {
            for flag in [false, true] {
                assert!(!role.reads_private(flag, false), "{role:?} with the setting off");
            }
        }
        assert!(Role::Dm.reads_private(false, true));
        assert!(!Role::Owner.reads_private(false, true), "being the owner alone reads nothing");
        assert!(Role::Owner.reads_private(true, true), "an owner who's also the DM");
        assert!(!Role::Player.reads_private(true, true));
    }

    #[test]
    fn private_file_id_known_answer() {
        // Frozen: changing this breaks every synced private note.
        let member = "aaaaaaaaaaaaaaaaaaaaaaaaaa";
        let path = "Sessions/Session 4/Sibling 5.md";
        assert_eq!(private_file_id(&KEY, member, path), PRIVATE_FILE_ID_KAT);
        assert!(is_id(PRIVATE_FILE_ID_KAT));
        // Never the shared id of the same path, nor of "Private/<path>", nor another member's.
        assert_ne!(private_file_id(&KEY, member, path), file_id(&KEY, path));
        assert_ne!(private_file_id(&KEY, member, path), file_id(&KEY, &format!("Private/{path}")));
        assert_ne!(private_file_id(&KEY, member, path), private_file_id(&KEY, "baaaaaaaaaaaaaaaaaaaaaaaaa", path));
        assert_ne!(private_file_id(&[8u8; 32], member, path), PRIVATE_FILE_ID_KAT);
    }
    const PRIVATE_FILE_ID_KAT: &str = "tlbg3wauz42hmvskw635gnxcv4";

    #[test]
    fn private_blobs_open_only_for_their_author_and_space() {
        let room = random_id();
        let (me, other) = (random_id(), random_id());
        let file = file();
        let id = private_file_id(&KEY, &me, &file.path);
        let blob = seal(&KEY, &room, &id, &file);
        assert_eq!(open_private(&KEY, &room, &me, &id, &blob), Ok(file.clone()));
        // Relabelled as another member's, or passed off as a shared file: it doesn't open.
        assert_eq!(open_private(&KEY, &room, &other, &id, &blob), Err(Error::Decrypt));
        assert_eq!(open(&KEY, &room, &id, &blob), Err(Error::Decrypt));
        // A shared blob doesn't open as private either.
        let shared_id = file_id(&KEY, &file.path);
        let shared = seal(&KEY, &room, &shared_id, &file);
        assert_eq!(open_private(&KEY, &room, &me, &shared_id, &shared), Err(Error::Decrypt));
        // An author that isn't a member id, and unsafe paths, are refused.
        assert_eq!(open_private(&KEY, &room, "../x", &id, &blob), Err(Error::Decrypt));
        let evil = FileContent { path: "../escape.md".into(), ..Default::default() };
        let evil_id = private_file_id(&KEY, &me, &evil.path);
        assert_eq!(open_private(&KEY, &room, &me, &evil_id, &seal(&KEY, &room, &evil_id, &evil)), Err(Error::Decrypt));
        assert_eq!(open_private(&[8u8; 32], &room, &me, &id, &blob), Err(Error::Decrypt), "wrong key");
    }
}
