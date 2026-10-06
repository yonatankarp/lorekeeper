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
    /// Wrong key, tampered blob, or a blob that belongs to another file or room.
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileContent {
    pub path: String,
    #[serde(with = "base64_bytes")]
    pub content: Vec<u8>,
    /// Unix milliseconds.
    pub modified: i64,
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

fn open_raw(key: &[u8; SECRET_LEN], aad: &[u8], blob: &[u8]) -> Result<Vec<u8>, Error> {
    if blob.len() < NONCE_LEN + TAG_LEN {
        return Err(Error::Decrypt);
    }
    let (nonce, ct) = blob.split_at(NONCE_LEN);
    let nonce: [u8; NONCE_LEN] = nonce.try_into().expect("split at NONCE_LEN");
    let cipher = XChaCha20Poly1305::new(&(*key).into());
    cipher.decrypt(&XNonce::from(nonce), Payload { msg: ct, aad }).map_err(|_| Error::Decrypt)
}

/// Decrypts a blob made by [`seal`] for the same key, room and id.
pub fn open(key: &[u8; SECRET_LEN], room: &str, id: &str, blob: &[u8]) -> Result<FileContent, Error> {
    let json = open_raw(key, &associated_data(room, id), blob)?;
    serde_json::from_slice(&json).map_err(|_| Error::Decrypt)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    Member,
}

/// `POST /v1/rooms` answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateRoomResponse {
    pub room: String,
    pub owner_token: String,
}

/// `POST /v1/rooms/{room}/invites` answer, and an item of `GET /v1/rooms/{room}/invites`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InviteInfo {
    pub invite: String,
    /// Unix milliseconds.
    pub expires: i64,
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
    /// Unix milliseconds.
    pub created: i64,
    pub last_seen: Option<i64>,
}

/// One file's latest version; `blob` is `None` for a deleted file (tombstone).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub id: String,
    pub seq: u64,
    pub blob: Option<String>,
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
    /// First message: replays changes after `since`, then streams live.
    Hello { since: u64, member: String },
    Put { req: u64, id: String, base: u64, blob: String },
    Delete { req: u64, id: String, base: u64 },
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
    /// A replay batch after `hello`; the last one has `more: false`.
    Changes { seq: u64, changes: Vec<Change>, more: bool },
    /// A live write by another connection.
    Change(Change),
    Presence { members: Vec<PresenceMember> },
    Ack { req: u64, seq: u64 },
    Conflict { req: u64, seq: u64, blob: Option<String> },
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
        FileContent { path: "Sessions/Session 4/Sibling 5.md".into(), content: b"# Notes\n".to_vec(), modified: 1_700_000_000_000 }
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
        let (room, id) = (random_id(), random_id());
        let a = seal(&KEY, &room, &id, &file());
        let b = seal(&KEY, &room, &id, &file());
        assert_ne!(a, b);
        assert_eq!(a.len(), NONCE_LEN + serde_json::to_vec(&file()).unwrap().len() + TAG_LEN);
        assert_eq!(open(&KEY, &room, &id, &a), Ok(file()));
        assert_eq!(open(&KEY, &room, &id, &decode_blob(&encode_blob(&b)).unwrap()), Ok(file()));
        let empty = FileContent { path: "a.md".into(), content: vec![], modified: 0 };
        assert_eq!(open(&KEY, &room, &id, &seal(&KEY, &room, &id, &empty)), Ok(empty));
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
        let change = Change { id: "i".into(), seq: 3, blob: None };
        assert_eq!(json(&ServerMessage::Change(change.clone())), r#"{"type":"change","id":"i","seq":3,"blob":null}"#);
        assert_eq!(json(&ServerMessage::Ack { req: 1, seq: 2 }), r#"{"type":"ack","req":1,"seq":2}"#);
        assert_eq!(json(&ServerMessage::Pong), r#"{"type":"pong"}"#);
        assert_eq!(json(&ServerMessage::Error { req: None, error: "x".into() }), r#"{"type":"error","error":"x"}"#);
        assert_eq!(json(&ServerMessage::Error { req: Some(4), error: "x".into() }), r#"{"type":"error","req":4,"error":"x"}"#);
        assert_eq!(
            json(&ServerMessage::Presence { members: vec![PresenceMember { member_id: "m".into(), member: "n".into(), role: Role::Owner }] }),
            r#"{"type":"presence","members":[{"member_id":"m","member":"n","role":"owner"}]}"#
        );
        assert_eq!(
            json(&ServerMessage::Changes { seq: 3, changes: vec![change], more: false }),
            r#"{"type":"changes","seq":3,"changes":[{"id":"i","seq":3,"blob":null}],"more":false}"#
        );
        let parse = |s: &str| serde_json::from_str::<ClientMessage>(s).unwrap();
        assert_eq!(parse(r#"{"type":"hello","since":0,"member":"x"}"#), ClientMessage::Hello { since: 0, member: "x".into() });
        assert_eq!(parse(r#"{"type":"ping"}"#), ClientMessage::Ping);
        assert_eq!(
            parse(r#"{"type":"put","req":1,"id":"i","base":0,"blob":"AA=="}"#),
            ClientMessage::Put { req: 1, id: "i".into(), base: 0, blob: "AA==".into() }
        );
        assert_eq!(parse(r#"{"type":"delete","req":1,"id":"i","base":2}"#), ClientMessage::Delete { req: 1, id: "i".into(), base: 2 });
        assert!(serde_json::from_str::<ClientMessage>(r#"{"type":"shout"}"#).is_err());
        assert_eq!(serde_json::to_string(&Role::Member).unwrap(), r#""member""#);
    }
}
