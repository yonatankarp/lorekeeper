//! Shared campaigns: the sync engine (docs/SYNC.md, "Client behaviour"). Its core knows nothing of Tauri: it gets a
//! campaign folder, a state file, the server, room, key and token, and reports through a [`Sink`]. lib.rs wires it to
//! the app (keychain, settings, watch.rs, events); the tests in sync_tests.rs drive engines against the real server.
//!
//! Everything that comes from the server or from other players is untrusted: paths are checked before anything is
//! written, symlinks are never followed, nothing is overwritten that changed here, and sizes are capped.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs;
use std::future::Future;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sync_protocol::{
    content_hash, decode_blob, decode_secret, encode_blob, file_id, open, open_member, random_id, seal, seal_member, Change,
    ClientMessage, CreateRoomResponse, FileContent, InviteInfo, MemberInfo, RedeemResponse, Role, ServerMessage,
    CREATE_KEY_HEADER, SECRET_LEN,
};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest, protocol::WebSocketConfig, Message};
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};
use zeroize::Zeroizing;

/// The maintainer's server; `LOREKEEPER_SYNC_SERVER` or the Sync server setting replaces it.
pub const DEFAULT_SERVER: &str = "https://lorekeeper.yonatankarp.com";
/// The one hidden file that syncs: `{"name": ...}`, written by the owner on Share, so joiners learn the name.
pub const METADATA: &str = ".lorekeeper/campaign.json";
/// The server's default blob limit (decoded `nonce || ciphertext || tag`).
const MAX_BLOB: usize = 30 * 1024 * 1024;
/// Bigger files aren't synced: their content goes into the blob as base64, inside JSON.
const MAX_FILE: u64 = (MAX_BLOB as u64 - 64 * 1024) / 4 * 3;
/// Largest WebSocket message accepted: a blob in base64 inside a JSON frame (the server's own cap).
const MAX_FRAME: usize = MAX_BLOB.div_ceil(3) * 4 + 64 * 1024;
/// A room bigger than the server's defaults stops syncing instead of filling the disk.
const MAX_FILES: usize = 20_000;
const MAX_ROOM_BYTES: u64 = 1024 * 1024 * 1024;
/// Data a run may write (files and conflict copies) before it stops, and conflict copies it may make.
const MAX_WRITTEN: u64 = 4 * MAX_ROOM_BYTES;
const MAX_CONFLICTS: usize = 500;
/// Writes in flight at once; the server allows 60 a minute per room.
const WINDOW: usize = 4;
const PING_EVERY: Duration = Duration::from_secs(30);
/// The server pings every 30 seconds; this long without a frame means the connection is gone (sleep, network).
const IDLE: Duration = Duration::from_secs(75);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const RATE_PAUSE: Duration = Duration::from_secs(20);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// Paths longer than this aren't synced (each name is at most 255 bytes).
const MAX_PATH: usize = 1024;
const SAVE_EVERY: Duration = Duration::from_secs(1);
/// More than the server's 32 members: the rest of a longer presence list is dropped.
const MAX_PRESENCE: usize = 64;

// ---------- what syncs ----------

/// Whether a path inside the campaign syncs: Markdown pages and images (the same list as the app) with names that
/// are safe on every system, never hidden files (except [`METADATA`]), the shared Templates/ folder, or sync
/// conflict copies. Used for local files and for every path that arrives from the network.
pub(crate) fn syncs(rel: &str) -> bool {
    if rel == METADATA {
        return true;
    }
    let parts: Vec<&str> = rel.split('/').collect();
    let names_ok = parts.iter().all(|p| p.len() <= 255 && !p.starts_with('.') && crate::restore::safe_part(p, true) && !deceptive(p));
    // Templates/ in any case: macOS and Windows would find the real one under "templates/" (APFS even "Templateſ/").
    if rel.len() > MAX_PATH || !names_ok || folded(parts[0]) == "templates" {
        return false;
    }
    let name = parts[parts.len() - 1];
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    (ext == "md" || crate::IMAGE_EXTS.contains(&ext.as_str())) && !is_conflict_copy(name)
}

/// A name as case-insensitive file systems compare it, near enough: `TEMPLATES`, `templates` and `Templateſ` agree.
pub(crate) fn folded(name: &str) -> String {
    name.to_uppercase().to_lowercase()
}

/// A name that shows as something else (bidi controls reorder it) or that Windows may read as another file's short
/// 8.3 name (`TEMPLA~1` is `Templates`).
fn deceptive(name: &str) -> bool {
    name.chars().any(|c| matches!(c, '\u{200e}' | '\u{200f}' | '\u{061c}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
        || name.as_bytes().windows(2).any(|w| w[0] == b'~' && w[1].is_ascii_digit())
}

/// A sync app's conflict copy, by its name: the named patterns of syncConflicts in vault.js ("conflicted copy",
/// "(... conflict ...)", "[Conflict]", ".sync-conflict-"). Keep the two in step; sync_tests.rs checks the same names.
pub(crate) fn is_conflict_copy(name: &str) -> bool {
    let lower = name.to_lowercase();
    if lower.contains("conflicted copy") || lower.contains(".sync-conflict-") {
        return true;
    }
    lower.char_indices().filter(|(_, c)| *c == '(' || *c == '[').any(|(i, _)| {
        let rest = &lower[i + 1..];
        let seg = &rest[..rest.find([')', ']']).unwrap_or(rest.len())];
        seg.match_indices("conflict").any(|(j, _)| j == 0 || !seg[..j].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_'))
    })
}

/// The name of the copy that keeps another player's version: `Vex (conflict 2026-10-06 2015).md`, or with a
/// counter (`... 2015 2).md`) when that exists. syncConflicts in vault.js lists it on Home.
fn conflict_name(rel: &str, when: &str, n: u32) -> String {
    let (dir, name) = rel.rsplit_once('/').map_or(("", rel), |(d, n)| (d, n));
    let (stem, ext) = name.rsplit_once('.').map_or((name, String::new()), |(s, e)| (s, format!(".{e}")));
    let counter = if n > 1 { format!(" {n}") } else { String::new() };
    let tail = format!(" (conflict {when}{counter}){ext}");
    // A name stays within 255 bytes: a long one loses the end of its stem, never the part that marks it a copy.
    let mut end = stem.len().min(255usize.saturating_sub(tail.len()));
    while !stem.is_char_boundary(end) {
        end -= 1;
    }
    let file = format!("{}{tail}", &stem[..end]);
    if dir.is_empty() { file } else { format!("{dir}/{file}") }
}

/// The campaign name in the metadata file's content, made safe as a folder name; None when it's unusable.
fn metadata_name(bytes: &[u8]) -> Option<String> {
    let v: Value = serde_json::from_slice(bytes).ok().filter(|_| bytes.len() < 64 * 1024)?;
    folder_name(v["name"].as_str()?)
}

/// A name from the network as a folder name: trimmed, at most 80 characters, and safe on every system.
pub(crate) fn folder_name(name: &str) -> Option<String> {
    let name: String = name.trim().chars().take(80).collect();
    let name = name.trim_end_matches(['.', ' ']).to_string();
    (!name.starts_with('.') && crate::restore::safe_part(&name, true) && !deceptive(&name)).then_some(name)
}

// ---------- state: path -> {id, seq, hash}, plus the room's last seq ----------

#[derive(Serialize, Deserialize, Default, Debug, Clone, PartialEq)]
pub(crate) struct State {
    /// The last seq received in `changes`/`change` (never from an ack: others' earlier writes may still be coming).
    pub seq: u64,
    pub files: BTreeMap<String, Entry>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub(crate) struct Entry {
    pub id: String,
    pub seq: u64,
    /// What this computer last synced, or [`UNSYNCED`] when the server's version isn't the party's (garbage or a
    /// delete without the key): yours then goes up over it.
    pub hash: String,
    pub len: u64,
    /// The file was deleted: its encrypted tombstone is the version at `seq`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub gone: bool,
}

/// A hash no content has: see Entry::hash.
const UNSYNCED: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Reads the state file; one that's missing, damaged or doesn't fit the key means a fresh catch-up (local files are
/// compared by hash, never deleted).
pub(crate) fn load_state(file: &Path, key: &[u8; SECRET_LEN]) -> State {
    let Some(state) = fs::read(file).ok().and_then(|b| serde_json::from_slice::<State>(&b).ok()) else { return State::default() };
    let valid = state.files.len() <= MAX_FILES
        && state.files.iter().all(|(path, e)| {
            syncs(path) && e.id == file_id(key, path) && e.hash.len() == 64 && e.hash.bytes().all(|b| b.is_ascii_hexdigit())
        });
    if valid { state } else { State::default() }
}

/// Writes through a temporary file and a rename, so a crash never leaves half a state file.
fn save_state(file: &Path, state: &State) -> io::Result<()> {
    fs::create_dir_all(file.parent().unwrap_or(Path::new(".")))?;
    let tmp = file.with_extension(format!("tmp-{}", &random_id()[..8]));
    fs::write(&tmp, serde_json::to_vec(state)?)?;
    fs::rename(&tmp, file).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

// ---------- the engine ----------

/// What the engine reports. Called from the engine's task; keep it quick.
pub trait Sink: Send + Sync + 'static {
    fn status(&self, status: &Status);
    /// Who else is connected: their names, decrypted.
    fn presence(&self, names: &[String]);
    /// Something the player should know (a file too big to sync, a refused change). Never contains file names or
    /// secrets.
    fn warning(&self, text: &str);
    /// About to write or remove `path` for a remote change (record it as the app's own write).
    fn wrote(&self, path: &Path);
    /// Remote changes landed on disk: refresh what's shown.
    fn changed(&self);
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Status {
    Connecting,
    /// Catching up (count 0) or sending `count` local changes.
    Syncing { count: usize },
    Synced,
    Offline,
    /// The owner removed this player (or the room is gone): sync stopped for good, local files kept.
    Removed,
    /// Sync stopped and won't retry; `reason` says why.
    Stopped { reason: String },
}

pub struct Config {
    pub root: PathBuf,
    pub state_file: PathBuf,
    /// `https://host[:port]` (see sync_protocol::server_origin).
    pub server: String,
    pub room: String,
    pub key: Zeroizing<[u8; SECRET_LEN]>,
    pub token: Zeroizing<String>,
    /// Your PC's name; sent sealed with the room key so only the party can read it.
    pub name: String,
}

enum Cmd {
    Poke,
    Stop,
}

/// Talks to a running engine. Dropping it stops the engine too.
pub struct Engine {
    tx: mpsc::UnboundedSender<Cmd>,
}

impl Engine {
    /// Local files may have changed: look and push.
    pub fn poke(&self) {
        let _ = self.tx.send(Cmd::Poke);
    }

    pub fn stop(&self) {
        let _ = self.tx.send(Cmd::Stop);
    }
}

/// An engine and the task that runs it; the caller spawns the task on its tokio runtime.
pub fn engine(cfg: Config, sink: Arc<dyn Sink>) -> (Engine, impl Future<Output = ()> + Send + 'static) {
    let (tx, rx) = mpsc::unbounded_channel();
    (Engine { tx }, run(cfg, sink, rx))
}

type Ws = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

enum End {
    Stop,
    Dropped,
    Removed,
    Fatal(String),
}

enum Local {
    Missing,
    /// A folder, a symlink, or a path through one: never read or written.
    NotRegular,
    TooBig,
    File(Vec<u8>, SystemTime),
}

enum Flight {
    Put { path: String, hash: String, len: u64 },
    Delete { path: String },
}

impl Flight {
    fn path(&self) -> &str {
        match self {
            Flight::Put { path, .. } | Flight::Delete { path } => path,
        }
    }
}

/// One connection's push side.
#[derive(Default)]
struct Push {
    queue: VecDeque<String>,
    inflight: HashMap<u64, Flight>,
    next_req: u64,
    replay_done: bool,
    rescan: bool,
    paused_until: Option<tokio::time::Instant>,
    /// The server refused more files (room full): nothing more is sent.
    blocked: bool,
    /// Conflicts per path since its last ack; past MAX_RETRIES the path waits for the next connection, so a
    /// server that keeps answering conflict can't make the engine spin.
    retries: HashMap<String, u32>,
}

const MAX_RETRIES: u32 = 5;

struct Core {
    cfg: Config,
    sink: Arc<dyn Sink>,
    /// The campaign folder, canonical; every write is checked to stay under it.
    root: PathBuf,
    /// The display id sent in hello: the name sealed with the room key (random nonce, so it also tells our own
    /// entry in presence apart).
    member: String,
    state: State,
    dirty: bool,
    touched: bool,
    /// A version from outside the party was ignored: look again, so ours goes back up over it.
    repair: bool,
    /// File id -> the highest seq applied, so a version that arrives twice (a live change and a conflict reply)
    /// is applied once.
    seen: HashMap<String, u64>,
    /// Path -> (size, modified, hash), so unchanged files aren't hashed again on every look.
    cache: HashMap<String, (u64, SystemTime, String)>,
    /// (path, hash) the server or the size limit refused; tried again once the file changes.
    skipped: HashSet<(String, String)>,
    warned: HashSet<&'static str>,
    written: u64,
    conflicts: usize,
    last_status: Option<Status>,
    saved: Instant,
}

async fn run(cfg: Config, sink: Arc<dyn Sink>, mut rx: mpsc::UnboundedReceiver<Cmd>) {
    let root = match cfg.root.canonicalize() {
        Ok(r) => r,
        Err(_) => {
            sink.status(&Status::Stopped { reason: "The campaign folder isn't there.".into() });
            return;
        }
    };
    let mut core = Core::new(cfg, sink, root);
    core.report(&Status::Connecting);
    let mut backoff = Duration::from_secs(1);
    loop {
        let began = Instant::now();
        let end = match connect(&core.cfg).await {
            Ok(ws) => core.session(ws, &mut rx).await,
            Err(Some(401)) => End::Removed,
            Err(_) => End::Dropped,
        };
        core.save();
        match end {
            End::Stop => return,
            End::Removed => return core.report(&Status::Removed),
            End::Fatal(reason) => return core.report(&Status::Stopped { reason }),
            End::Dropped => core.report(&Status::Offline),
        }
        // Only a connection that lasted starts the waits over, so a server that keeps closing (or refusing: the
        // server allows 60 connections per player a minute) is never hammered.
        if began.elapsed() > Duration::from_secs(60) {
            backoff = Duration::from_secs(1);
        }
        // ponytail: plain doubling up to a minute, no jitter; a party's handful of clients won't stampede.
        let wake = tokio::time::sleep(backoff);
        tokio::pin!(wake);
        loop {
            tokio::select! {
                _ = &mut wake => break,
                cmd = rx.recv() => if matches!(cmd, None | Some(Cmd::Stop)) { return },
            }
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// rustls with ring (as ureq) and the Mozilla roots; certificates are always verified.
static TLS: LazyLock<Arc<rustls::ClientConfig>> = LazyLock::new(|| {
    let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("ring supports the default TLS versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    Arc::new(config)
});

impl Core {
    /// `root` is the campaign folder, canonical.
    fn new(cfg: Config, sink: Arc<dyn Sink>, root: PathBuf) -> Core {
        let state = load_state(&cfg.state_file, &cfg.key);
        let member = seal_member(&cfg.key, &cfg.room, &cfg.name);
        Core {
            cfg,
            sink,
            root,
            member,
            state,
            dirty: false,
            touched: false,
            repair: false,
            seen: HashMap::new(),
            cache: HashMap::new(),
            skipped: HashSet::new(),
            warned: HashSet::new(),
            written: 0,
            conflicts: 0,
            last_status: None,
            saved: Instant::now(),
        }
    }

    fn report(&mut self, status: &Status) {
        if self.last_status.as_ref() != Some(status) {
            self.sink.status(status);
            self.last_status = Some(status.clone());
        }
    }

    fn warn(&mut self, text: &'static str) {
        if self.warned.insert(text) {
            self.sink.warning(text);
        }
    }

    fn save(&mut self) {
        if self.dirty && save_state(&self.cfg.state_file, &self.state).is_ok() {
            self.dirty = false;
            self.saved = Instant::now();
        }
    }

    async fn session(&mut self, mut ws: Ws, rx: &mut mpsc::UnboundedReceiver<Cmd>) -> End {
        // Each connection starts from the state on disk; anything applied twice is caught by the hashes.
        self.seen = self.state.files.values().map(|e| (e.id.clone(), e.seq)).collect();
        let hello = ClientMessage::Hello { since: self.state.seq, member: self.member.clone() };
        if send(&mut ws, &hello).await.is_err() {
            return End::Dropped;
        }
        self.report(&Status::Syncing { count: 0 });
        let mut s = Push { next_req: 1, ..Push::default() };
        let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + PING_EVERY, PING_EVERY);
        let mut last_heard = Instant::now();
        loop {
            let paused = s.paused_until;
            let pause = async move {
                match paused {
                    Some(t) => tokio::time::sleep_until(t).await,
                    None => std::future::pending().await,
                }
            };
            // Unsaved state waits at most SAVE_EVERY, even when nothing else happens.
            let flush_at = self.dirty.then(|| tokio::time::Instant::from_std(self.saved + SAVE_EVERY));
            let flush = async move {
                match flush_at {
                    Some(t) => tokio::time::sleep_until(t).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                msg = ws.next() => {
                    let Some(Ok(msg)) = msg else { return End::Dropped };
                    last_heard = Instant::now();
                    match msg {
                        Message::Text(text) => {
                            // Unknown messages are ignored, so a newer server can add some.
                            if let Ok(m) = serde_json::from_str::<ServerMessage>(text.as_str()) {
                                if let Err(end) = self.handle(m, &mut s) {
                                    return end;
                                }
                                s.rescan |= std::mem::take(&mut self.repair);
                            }
                        }
                        Message::Close(frame) => {
                            return if frame.is_some_and(|f| u16::from(f.code) == 4001) { End::Removed } else { End::Dropped };
                        }
                        _ => {}
                    }
                }
                cmd = rx.recv() => match cmd {
                    Some(Cmd::Poke) => s.rescan = true,
                    None | Some(Cmd::Stop) => {
                        let _ = ws.close(None).await;
                        return End::Stop;
                    }
                },
                _ = ping.tick() => {
                    if last_heard.elapsed() > IDLE || send(&mut ws, &ClientMessage::Ping).await.is_err() {
                        return End::Dropped;
                    }
                }
                _ = pause => s.paused_until = None,
                _ = flush => {}
            }
            if let Err(end) = self.pump(&mut ws, &mut s).await {
                return end;
            }
            if self.touched {
                self.touched = false;
                self.sink.changed();
            }
            // At most once a second while connected (and at the end of each connection): a server sending tiny
            // frames can't make every one of them rewrite the whole state file.
            if self.saved.elapsed() >= SAVE_EVERY {
                self.save();
            }
            let count = s.queue.len() + s.inflight.len();
            let status = if !s.replay_done {
                Status::Syncing { count: 0 }
            } else if count > 0 {
                Status::Syncing { count }
            } else {
                Status::Synced
            };
            self.report(&status);
        }
    }

    /// One message from the server. Err ends the connection.
    fn handle(&mut self, msg: ServerMessage, s: &mut Push) -> Result<(), End> {
        match msg {
            ServerMessage::Changes { changes, more, .. } => {
                for c in changes {
                    self.apply(c, true)?;
                }
                if !more {
                    s.replay_done = true;
                    s.rescan = true;
                }
            }
            ServerMessage::Change(c) => self.apply(c, true)?,
            ServerMessage::Presence { members } => {
                let mut names: Vec<String> = members
                    .iter()
                    .filter(|m| m.member != self.member && m.member.len() <= 512)
                    .take(MAX_PRESENCE)
                    .map(|m| {
                        let name = open_member(&self.cfg.key, &self.cfg.room, &m.member).unwrap_or_default();
                        let name: String = name.chars().filter(|c| !c.is_control()).take(60).collect();
                        if !name.trim().is_empty() {
                            name
                        } else if m.role == Role::Owner {
                            "the owner".into()
                        } else {
                            "a player".into()
                        }
                    })
                    .collect();
                names.sort_by_key(|n| n.to_lowercase());
                self.sink.presence(&names);
            }
            ServerMessage::Ack { req, seq } => match s.inflight.remove(&req).inspect(|f| {
                s.retries.remove(f.path());
            }) {
                Some(Flight::Put { path, hash, len }) => {
                    let id = file_id(&self.cfg.key, &path);
                    if self.seen.get(&id).is_none_or(|&s| s < seq) {
                        self.seen.insert(id.clone(), seq);
                        self.state.files.insert(path, Entry { id, seq, hash, len, gone: false });
                        self.dirty = true;
                    }
                }
                Some(Flight::Delete { path }) => {
                    let id = file_id(&self.cfg.key, &path);
                    if self.seen.get(&id).is_none_or(|&s| s < seq) {
                        self.seen.insert(id.clone(), seq);
                        self.state.files.insert(path, Entry { id, seq, hash: UNSYNCED.into(), len: 0, gone: true });
                        self.dirty = true;
                    }
                }
                None => {}
            },
            ServerMessage::Conflict { req, seq, blob } => {
                if let Some(f) = s.inflight.remove(&req) {
                    let path = f.path().to_string();
                    let id = file_id(&self.cfg.key, &path);
                    self.apply(Change { id: id.clone(), seq, blob }, false)?;
                    // Whatever that version was (even one not from the party, or one already seen), the next push
                    // builds on it.
                    match self.state.files.get_mut(&path) {
                        Some(e) if e.seq < seq => {
                            e.seq = seq;
                            if !e.gone {
                                e.hash = UNSYNCED.into();
                            }
                        }
                        Some(_) => {}
                        None => {
                            let entry = Entry { id, seq, hash: UNSYNCED.into(), len: 0, gone: false };
                            self.state.files.insert(path.clone(), entry);
                        }
                    }
                    self.dirty = true;
                    let retries = s.retries.entry(path).or_default();
                    *retries += 1;
                    if *retries > MAX_RETRIES {
                        self.warn("A file keeps conflicting on the sync server; it waits until the next connection.");
                    }
                    s.rescan = true; // what's still different here goes again, over the newer version
                }
            }
            ServerMessage::Error { req: Some(req), error } => {
                if let Some(f) = s.inflight.remove(&req) {
                    match error.as_str() {
                        "rate_limited" | "internal" => {
                            s.queue.push_front(f.path().to_string());
                            s.paused_until = Some(tokio::time::Instant::now() + RATE_PAUSE);
                        }
                        "room_full" | "too_many_files" => {
                            s.blocked = true;
                            s.queue.clear();
                            self.warn("The campaign is as big as the sync server allows; new changes aren't synced.");
                        }
                        _ => {
                            if let Flight::Put { path, hash, .. } = f {
                                self.skipped.insert((path, hash));
                            }
                            self.warn("The sync server refused a file; it stays on this computer only.");
                        }
                    }
                }
            }
            // An error without a request (a refused hello): try again later.
            ServerMessage::Error { req: None, .. } => return Err(End::Dropped),
            ServerMessage::Pong => {}
        }
        Ok(())
    }

    /// Sends queued changes, and looks for new ones once the queue is empty.
    async fn pump(&mut self, ws: &mut Ws, s: &mut Push) -> Result<(), End> {
        if !s.replay_done || s.blocked {
            return Ok(());
        }
        if s.rescan && s.queue.is_empty() && s.inflight.is_empty() {
            s.rescan = false;
            s.queue = self.scan()?.into();
        }
        while s.paused_until.is_none() && s.inflight.len() < WINDOW {
            let Some(path) = s.queue.pop_front() else { break };
            if s.retries.get(&path).is_some_and(|&n| n > MAX_RETRIES) {
                continue;
            }
            let req = s.next_req;
            if let Some((msg, flight)) = self.prepare(&path, req) {
                if send(ws, &msg).await.is_err() {
                    return Err(End::Dropped);
                }
                s.inflight.insert(req, flight);
                s.next_req += 1;
            }
        }
        Ok(())
    }

    /// Every local path that differs from the state: new or changed files, and synced files that are gone.
    fn scan(&mut self) -> Result<Vec<String>, End> {
        if !self.root.is_dir() {
            self.warn("The campaign folder isn't there, so nothing is synced until it's back.");
            return Ok(Vec::new());
        }
        let mut disk = BTreeMap::new();
        if walk(&self.root, "", &mut disk) > 0 {
            self.warn("Some pages or images have names that don't work on every computer (a ? or :, or a name like CON) and stay on this computer only.");
        }
        if disk.len() > MAX_FILES {
            return Err(End::Fatal("The campaign has more files than sync handles (20,000).".into()));
        }
        let mut out = Vec::new();
        for (rel, (len, modified)) in &disk {
            if *len > MAX_FILE {
                self.warn("A file is over the sync size limit (about 22 MB) and stays on this computer only.");
                continue;
            }
            let hash = match self.cache.get(rel) {
                Some((l, m, h)) if l == len && m == modified => h.clone(),
                _ => match self.read_local(rel) {
                    Ok(Local::File(bytes, _)) => {
                        let h = content_hash(&bytes);
                        self.cache.insert(rel.clone(), (*len, *modified, h.clone()));
                        h
                    }
                    _ => continue,
                },
            };
            let synced = self.state.files.get(rel).is_some_and(|e| !e.gone && e.hash == hash);
            if !synced && !self.skipped.contains(&(rel.clone(), hash)) {
                out.push(rel.clone());
            }
        }
        self.cache.retain(|rel, _| disk.contains_key(rel));
        // A synced metadata file that's gone means this may not be the campaign's real folder (a drive that isn't
        // mounted, an emptied folder): never turn that into deleting everyone's files.
        let lost_metadata = self.state.files.get(METADATA).is_some_and(|e| !e.gone) && !disk.contains_key(METADATA);
        if lost_metadata {
            self.warn("The campaign folder looks incomplete, so deletions aren't synced.");
        } else {
            out.extend(self.state.files.iter().filter(|(p, e)| !e.gone && !disk.contains_key(*p)).map(|(p, _)| p.clone()));
        }
        Ok(out)
    }

    /// The message for one queued path, worked out now (the file may have changed since the scan).
    fn prepare(&mut self, path: &str, req: u64) -> Option<(ClientMessage, Flight)> {
        let entry = self.state.files.get(path);
        let (id, base) = (file_id(&self.cfg.key, path), entry.map_or(0, |e| e.seq));
        match self.read_local(path) {
            Ok(Local::File(content, modified)) => {
                let hash = content_hash(&content);
                if entry.is_some_and(|e| !e.gone && e.hash == hash) || self.skipped.contains(&(path.to_string(), hash.clone())) {
                    return None;
                }
                let len = content.len() as u64;
                let modified = modified.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
                let file = FileContent { path: path.to_string(), content, modified, deleted: false };
                let sealed = seal(&self.cfg.key, &self.cfg.room, &id, &file);
                if sealed.len() > MAX_BLOB {
                    self.skipped.insert((path.to_string(), hash));
                    self.warn("A file is over the sync size limit (about 22 MB) and stays on this computer only.");
                    return None;
                }
                let msg = ClientMessage::Put { req, id, base, blob: encode_blob(&sealed) };
                Some((msg, Flight::Put { path: path.to_string(), hash, len }))
            }
            // Deleted here: an encrypted tombstone, never the server's own delete (which anyone with a token
            // could send, so other players ignore it).
            Ok(Local::Missing) if entry.is_some_and(|e| !e.gone) && self.root.is_dir() && path != METADATA => {
                let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
                let blob = encode_blob(&seal(&self.cfg.key, &self.cfg.room, &id, &FileContent::tombstone(path, now)));
                Some((ClientMessage::Put { req, id, base, blob }, Flight::Delete { path: path.to_string() }))
            }
            _ => None,
        }
    }

    /// A remote version of a file: from the replay or live (`advance`: it moves the room's seq), or a conflict
    /// reply. Err(Dropped) on a disk error, so the change comes again after a reconnect; a name this computer can't
    /// hold (too long for its file system) is skipped instead, or that one change would stop sync for good.
    fn apply(&mut self, c: Change, advance: bool) -> Result<(), End> {
        if self.seen.get(&c.id).is_none_or(|&seen| seen < c.seq) {
            // An id the state doesn't know needs nothing when its version isn't the party's, and isn't remembered,
            // so a server can't grow memory with made-up ids.
            let mut remember = true;
            let result = match c.blob.as_deref().map(|blob| self.open_blob(&c.id, blob)) {
                Some(Some(file)) if !file.deleted => self.remote_put(file, &c.id, c.seq),
                // The metadata file is never deleted: joiners need it, and its absence pauses deletions.
                Some(Some(file)) if file.path != METADATA => self.remote_delete(&file.path, &c.id, c.seq),
                // Not sealed by the party: a server-side delete (anyone with a token can send one) or a blob that
                // doesn't open. Local files stay as they are, and yours goes back up over it.
                untrusted => {
                    if untrusted.is_none() {
                        self.warn("Ignored a deletion that didn't come from anyone in the party.");
                    }
                    // A conflict reply is always about one of ours.
                    remember = self.seen.contains_key(&c.id) || !advance;
                    // ponytail: a linear search per untrusted change of a known file; an id index if rooms grow.
                    let known = if remember { self.state.files.values_mut().find(|e| e.id == c.id) } else { None };
                    if let Some(e) = known {
                        e.seq = c.seq;
                        if !e.gone {
                            e.hash = UNSYNCED.into();
                            self.repair = true;
                        }
                    }
                    Ok(())
                }
            };
            match result {
                Ok(()) => {}
                Err(Fail::Io(e)) if e.kind() == io::ErrorKind::InvalidFilename => {
                    self.warn("Skipped a change whose name is too long for this computer.");
                }
                Err(Fail::Io(_)) => {
                    self.warn("Couldn't write a change to the campaign folder; trying again shortly.");
                    return Err(End::Dropped);
                }
                Err(Fail::Fatal(reason)) => return Err(End::Fatal(reason)),
            }
            if remember {
                self.seen.insert(c.id, c.seq);
            }
        }
        if advance && c.seq > self.state.seq {
            self.state.seq = c.seq;
        }
        self.dirty = true;
        Ok(())
    }

    /// Decrypts a change and checks it: its id must be its path's, and the path must be one that syncs. Anything
    /// else is skipped with a warning that names no file.
    fn open_blob(&mut self, id: &str, blob: &str) -> Option<FileContent> {
        let Some(bytes) = (blob.len() <= MAX_FRAME).then(|| decode_blob(blob).ok()).flatten() else {
            self.warn("Skipped a change that couldn't be read.");
            return None;
        };
        // open() also checks that the path inside is safe and is the one the id was made from.
        let Ok(file) = open(&self.cfg.key, &self.cfg.room, id, &bytes) else {
            self.warn("Skipped a change that couldn't be decrypted; it didn't come from anyone in the party.");
            return None;
        };
        if !syncs(&file.path) {
            self.warn("Skipped a file of a kind Lorekeeper doesn't sync.");
            return None;
        }
        Some(file)
    }

    fn remote_put(&mut self, file: FileContent, id: &str, seq: u64) -> Result<(), Fail> {
        let _guard = crate::WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let hash = content_hash(&file.content);
        let len = file.content.len() as u64;
        let entry = Entry { id: id.to_string(), seq, hash: hash.clone(), len, gone: false };
        let synced_hash = self.state.files.get(&file.path).filter(|e| !e.gone).map(|e| e.hash.clone());
        let local_unchanged = match self.read_local(&file.path)? {
            Local::NotRegular => {
                self.warn("Skipped a change to something that isn't a plain file here.");
                return Ok(());
            }
            Local::File(bytes, _) if content_hash(&bytes) == hash => {
                self.state.files.insert(file.path, entry);
                return Ok(());
            }
            Local::Missing => true,
            Local::File(bytes, _) => synced_hash.as_deref() == Some(content_hash(&bytes).as_str()),
            Local::TooBig => false,
        };
        let new_file = !self.state.files.contains_key(&file.path);
        if new_file && self.state.files.len() >= MAX_FILES {
            return Err(Fail::Fatal("The campaign has more files than sync handles (20,000).".into()));
        }
        let total: u64 = self.state.files.values().map(|e| e.len).sum::<u64>() + len;
        self.written += len;
        if total > MAX_ROOM_BYTES || self.written > MAX_WRITTEN {
            return Err(Fail::Fatal("The campaign is bigger than sync handles (1 GB).".into()));
        }
        if local_unchanged {
            if !self.write_file(&file.path, &file.content)? {
                self.warn("Skipped a change to something that isn't a plain file here.");
                return Ok(());
            }
        } else {
            // Changed here too: keep yours, and save theirs beside it. Yours then goes up over theirs.
            self.conflicts += 1;
            if self.conflicts > MAX_CONFLICTS {
                return Err(Fail::Fatal("Too many sync conflicts at once; sync stopped to be safe.".into()));
            }
            self.write_conflict_copy(&file.path, &file.content)?;
        }
        self.state.files.insert(file.path, entry);
        Ok(())
    }

    /// Another player deleted a file (an encrypted tombstone).
    fn remote_delete(&mut self, path: &str, id: &str, seq: u64) -> Result<(), Fail> {
        let _guard = crate::WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let synced_hash = self.state.files.get(path).filter(|e| !e.gone).map(|e| e.hash.clone());
        // Only an unchanged plain file goes; one changed here stays (a conflict) and goes up again over the tombstone.
        if let Local::File(bytes, _) = self.read_local(path)? {
            if synced_hash.as_deref() == Some(content_hash(&bytes).as_str()) {
                self.trash(path)?;
            }
        }
        self.state.files.insert(path.to_string(), Entry { id: id.into(), seq, hash: UNSYNCED.into(), len: 0, gone: true });
        Ok(())
    }

    /// Reads a file in the campaign without following symlinks, the file's or its folders'.
    fn read_local(&self, rel: &str) -> io::Result<Local> {
        let mut path = self.root.clone();
        for part in rel.split('/') {
            path.push(part);
            match fs::symlink_metadata(&path) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Local::Missing),
                Err(e) => return Err(e),
                Ok(m) if m.file_type().is_symlink() => return Ok(Local::NotRegular),
                Ok(_) => {}
            }
        }
        let meta = fs::symlink_metadata(&path)?;
        if !meta.is_file() {
            return Ok(Local::NotRegular);
        }
        if meta.len() > MAX_FILE {
            return Ok(Local::TooBig);
        }
        let file = match open_no_follow(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Local::Missing),
            Err(_) => return Ok(Local::NotRegular),
        };
        let mut content = Vec::with_capacity(meta.len() as usize);
        file.take(MAX_FILE + 1).read_to_end(&mut content)?;
        if content.len() as u64 > MAX_FILE {
            return Ok(Local::TooBig);
        }
        Ok(Local::File(content, meta.modified().unwrap_or(UNIX_EPOCH)))
    }

    /// Where `rel` may be written: its folders are made one by one, none may be a symlink or a file, the target may
    /// only be a plain file, and the result must stay under the campaign folder. None when refused.
    fn target(&self, rel: &str) -> io::Result<Option<PathBuf>> {
        let parts: Vec<&str> = rel.split('/').collect();
        let mut dir = self.root.clone();
        for part in &parts[..parts.len() - 1] {
            dir.push(part);
            match fs::symlink_metadata(&dir) {
                Ok(m) if m.is_dir() => {}
                Ok(_) => return Ok(None),
                Err(e) if e.kind() == io::ErrorKind::NotFound => match fs::create_dir(&dir) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists && fs::symlink_metadata(&dir)?.is_dir() => {}
                    Err(e) => return Err(e),
                },
                Err(e) => return Err(e),
            }
        }
        if !dir.canonicalize()?.starts_with(&self.root) {
            return Ok(None);
        }
        let target = dir.join(parts[parts.len() - 1]);
        match fs::symlink_metadata(&target) {
            Ok(m) if !m.is_file() => Ok(None),
            _ => Ok(Some(target)),
        }
    }

    /// Writes a remote file through a temporary file and a rename in its folder. False when refused (see target).
    fn write_file(&mut self, rel: &str, content: &[u8]) -> Result<bool, Fail> {
        let Some(target) = self.target(rel)? else { return Ok(false) };
        // A short name of its own: one built from the target's could pass the 255-byte limit the target is within.
        let tmp = target.with_file_name(format!(".lorekeeper-{}.tmp", &random_id()[..8]));
        let written = fs::OpenOptions::new().write(true).create_new(true).open(&tmp).and_then(|mut f| f.write_all(content));
        self.sink.wrote(&target);
        if let Err(e) = written.and_then(|_| fs::rename(&tmp, &target)) {
            let _ = fs::remove_file(&tmp);
            return Err(Fail::Io(e));
        }
        self.touched = true;
        Ok(true)
    }

    /// Saves another player's version beside yours under a name that can't exist yet (create_new).
    fn write_conflict_copy(&mut self, rel: &str, content: &[u8]) -> Result<(), Fail> {
        let when = chrono::Local::now().format("%Y-%m-%d %H%M").to_string();
        for n in 1..=100 {
            let name = conflict_name(rel, &when, n);
            let Some(target) = self.target(&name)? else { return Ok(()) };
            match fs::OpenOptions::new().write(true).create_new(true).open(&target) {
                Ok(mut f) => {
                    self.sink.wrote(&target);
                    f.write_all(content)?;
                    self.touched = true;
                    return Ok(());
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(Fail::Io(e)),
            }
        }
        Err(Fail::Io(io::ErrorKind::AlreadyExists.into())) // a minute later the names are free again
    }

    /// Moves a file another player deleted into the campaign's hidden .trash folder (Obsidian's), so it can be
    /// brought back; empty folders it leaves behind go, but never the campaign's top-level folders.
    fn trash(&mut self, rel: &str) -> Result<(), Fail> {
        let Some(from) = self.target(rel)? else { return Ok(()) };
        let (stem, ext) = rel.rsplit_once('.').unwrap_or((rel, ""));
        let mut n = 1;
        let to = loop {
            let name = if n == 1 { format!(".trash/{rel}") } else { format!(".trash/{stem} {n}.{ext}") };
            // Through target() too: a .trash that's a symlink (or has one on the way) is never followed.
            let Some(to) = self.target(&name)? else {
                self.warn("Skipped a change to something that isn't a plain file here.");
                return Ok(());
            };
            if fs::symlink_metadata(&to).is_err() {
                break to;
            }
            n += 1;
        };
        self.sink.wrote(&from);
        fs::rename(&from, &to)?;
        self.touched = true;
        let mut dir = from.parent();
        while let Some(d) = dir.filter(|d| d.parent().is_some_and(|p| p != self.root) && d.starts_with(&self.root)) {
            if fs::remove_dir(d).is_err() {
                break;
            }
            dir = d.parent();
        }
        Ok(())
    }
}

/// Opens the socket. Err(Some(status)) when the server answered the upgrade with an HTTP error.
async fn connect(cfg: &Config) -> Result<Ws, Option<u16>> {
    let (ws_url, tls) = match cfg.server.split_once("://") {
        Some(("https", host)) => (format!("wss://{host}"), true),
        Some(("http", host)) => (format!("ws://{host}"), false),
        _ => return Err(None),
    };
    let mut req = format!("{ws_url}/v1/rooms/{}/live", cfg.room).into_client_request().map_err(|_| None)?;
    let auth = format!("Bearer {}", cfg.token.as_str()).parse().map_err(|_| None)?;
    req.headers_mut().insert("authorization", auth);
    let config = WebSocketConfig::default().max_message_size(Some(MAX_FRAME)).max_frame_size(Some(MAX_FRAME));
    let connector = if tls { Connector::Rustls(TLS.clone()) } else { Connector::Plain };
    let connecting = tokio_tungstenite::connect_async_tls_with_config(req, Some(config), true, Some(connector));
    match tokio::time::timeout(CONNECT_TIMEOUT, connecting).await {
        Ok(Ok((ws, _))) => Ok(ws),
        Ok(Err(tungstenite::Error::Http(res))) => Err(Some(res.status().as_u16())),
        _ => Err(None),
    }
}


enum Fail {
    Io(io::Error),
    Fatal(String),
}

impl From<io::Error> for Fail {
    fn from(e: io::Error) -> Self {
        Fail::Io(e)
    }
}

/// Sends one message. A server that stops reading can't hold the engine (and its Stop) forever: past 30 seconds
/// plus 64 KiB a second for the message's size, the connection counts as dropped.
async fn send(ws: &mut Ws, msg: &ClientMessage) -> Result<(), tungstenite::Error> {
    let text = serde_json::to_string(msg).expect("ClientMessage serializes");
    let limit = Duration::from_secs(30 + text.len() as u64 / 65_536);
    tokio::time::timeout(limit, ws.send(Message::Text(text.into())))
        .await
        .unwrap_or_else(|_| Err(tungstenite::Error::Io(io::ErrorKind::TimedOut.into())))
}

/// Every file under `root` that syncs, with its size and time. Symlinks are skipped, never followed.
/// Returns how many pages and images were left out for their names alone.
fn walk(dir: &Path, prefix: &str, out: &mut BTreeMap<String, (u64, SystemTime)>) -> usize {
    let mut unsafe_names = 0;
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let (Ok(kind), Some(name)) = (entry.file_type(), entry.file_name().to_str().map(str::to_owned)) else { continue };
        let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        if kind.is_dir() {
            let wanted = rel == ".lorekeeper" || (!name.starts_with('.') && !(prefix.is_empty() && folded(&name) == "templates"));
            if wanted && out.len() <= MAX_FILES {
                unsafe_names += walk(&entry.path(), &rel, out);
            }
        } else if kind.is_file() && syncs(&rel) {
            if let Ok(meta) = entry.metadata() {
                out.insert(rel, (meta.len(), meta.modified().unwrap_or(UNIX_EPOCH)));
            }
        } else if kind.is_file() && !name.starts_with('.') && !is_conflict_copy(&name) {
            let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
            unsafe_names += usize::from(ext == "md" || crate::IMAGE_EXTS.contains(&ext.as_str()));
        }
    }
    unsafe_names
}

#[cfg(unix)]
fn open_no_follow(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(path)
}

/// Windows has no O_NOFOLLOW: the file is opened as itself even when it's a symlink or junction
/// (FILE_FLAG_OPEN_REPARSE_POINT), and refused when it is one, so one swapped in after read_local's check isn't
/// followed. (A folder on the way swapped for a junction in between is the remaining race, as on other systems.)
#[cfg(windows)]
fn open_no_follow(path: &Path) -> io::Result<fs::File> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    let file = fs::OpenOptions::new().read(true).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).open(path)?;
    if file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::other("a symlink or junction"));
    }
    Ok(file)
}

/// Joining: reads the replay just until the campaign's metadata file and returns its name (None when it isn't
/// there or can't be read in time). Nothing is written.
pub async fn fetch_name(server: &str, room: &str, key: &[u8; SECRET_LEN], token: &str) -> Option<String> {
    let cfg = Config {
        root: PathBuf::new(),
        state_file: PathBuf::new(),
        server: server.into(),
        room: room.into(),
        key: Zeroizing::new(*key),
        token: Zeroizing::new(token.into()),
        name: String::new(),
    };
    let core_connect = async {
        let mut ws = connect(&cfg).await.ok()?;
        send(&mut ws, &ClientMessage::Hello { since: 0, member: String::new() }).await.ok()?;
        let id = file_id(key, METADATA);
        while let Some(Ok(msg)) = ws.next().await {
            let Message::Text(text) = msg else { continue };
            let Ok(ServerMessage::Changes { changes, more, .. }) = serde_json::from_str(text.as_str()) else { continue };
            let found = changes.into_iter().find(|c| c.id == id).and_then(|c| c.blob);
            if let Some(blob) = found {
                let _ = ws.close(None).await;
                let file = open(key, room, &id, &decode_blob(&blob).ok()?).ok()?;
                return metadata_name(&file.content);
            }
            if !more {
                break;
            }
        }
        let _ = ws.close(None).await;
        None
    };
    tokio::time::timeout(Duration::from_secs(30), core_connect).await.ok().flatten()
}

// ---------- the HTTP API (rooms, invites, members) ----------

/// No redirects (a token or creation key must never reach another host) and firm timeouts. Replies of any status
/// come back as Ok.
static AGENT: LazyLock<ureq::Agent> = LazyLock::new(|| {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .user_agent("Lorekeeper")
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into()
});

/// A refused or failed request: the HTTP status (0 when the server couldn't be reached) and the server's code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub status: u16,
    pub code: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match (self.status, self.code.as_str()) {
            (0, _) => "Can't reach the sync server. Check your internet connection and try again.",
            (_, "unauthorized") => "The sync server doesn't accept this campaign's sign-in anymore.",
            (_, "owner_only") => "Only the player who shared this campaign can do that.",
            (_, "create_key_required") => "This sync server needs its creation key to share a campaign.",
            (_, "invite_not_found") => "This invite link isn't valid anymore (it may have been cancelled). Ask for a new one.",
            (_, "invite_used") => "This invite link was already used. Ask for a new one.",
            (_, "invite_expired") => "This invite link has expired. Ask for a new one.",
            (_, "too_many_members") => "This campaign has as many players and invites as it can (32).",
            (_, "rate_limited") => "Too many tries for now. Wait a while and try again.",
            (_, "cannot_remove_owner") => "The campaign's owner can't be removed.",
            (_, "not_found") => "That player isn't in the campaign anymore.",
            _ => return write!(f, "The sync server refused ({}).", self.status),
        };
        f.write_str(text)
    }
}

fn api(
    method: &str,
    url: &str,
    token: Option<&str>,
    create_key: Option<&str>,
    body: Option<Value>,
) -> Result<Value, ApiError> {
    let net = |_| ApiError { status: 0, code: String::new() };
    let auth = token.map(|t| format!("Bearer {t}"));
    let resp = match method {
        "GET" | "DELETE" => {
            let mut req = if method == "GET" { AGENT.get(url) } else { AGENT.delete(url) };
            if let Some(a) = &auth {
                req = req.header("Authorization", a);
            }
            req.call()
        }
        _ => {
            let mut req = AGENT.post(url);
            if let Some(a) = &auth {
                req = req.header("Authorization", a);
            }
            if let Some(k) = create_key {
                req = req.header(CREATE_KEY_HEADER, k);
            }
            match body {
                Some(b) => req.send_json(b),
                None => req.send_empty(),
            }
        }
    };
    let mut resp = resp.map_err(net)?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().with_config().limit(1 << 20).read_to_string().map_err(net)?;
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if (200..300).contains(&status) {
        Ok(v)
    } else {
        Err(ApiError { status, code: v["error"].as_str().unwrap_or_default().to_string() })
    }
}

fn parse<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, ApiError> {
    serde_json::from_value(v).map_err(|_| ApiError { status: 502, code: "bad_reply".into() })
}

/// `POST /v1/rooms`. Blocking (ureq); run it off the main thread.
pub fn create_room(server: &str, create_key: Option<&str>) -> Result<CreateRoomResponse, ApiError> {
    let r: CreateRoomResponse = parse(api("POST", &format!("{server}/v1/rooms"), None, create_key, None)?)?;
    if !sync_protocol::is_id(&r.room) || decode_secret(&r.owner_token).is_err() {
        return Err(ApiError { status: 502, code: "bad_reply".into() });
    }
    Ok(r)
}

pub fn create_invite(server: &str, room: &str, token: &str) -> Result<InviteInfo, ApiError> {
    let r: InviteInfo = parse(api("POST", &format!("{server}/v1/rooms/{room}/invites"), Some(token), None, None)?)?;
    if !sync_protocol::is_id(&r.invite) {
        return Err(ApiError { status: 502, code: "bad_reply".into() });
    }
    Ok(r)
}

pub fn list_invites(server: &str, room: &str, token: &str) -> Result<Vec<InviteInfo>, ApiError> {
    parse(api("GET", &format!("{server}/v1/rooms/{room}/invites"), Some(token), None, None)?)
}

pub fn cancel_invite(server: &str, room: &str, token: &str, invite: &str) -> Result<(), ApiError> {
    api("DELETE", &format!("{server}/v1/rooms/{room}/invites/{invite}"), Some(token), None, None).map(|_| ())
}

pub fn redeem(server: &str, room: &str, invite: &str, member: &str) -> Result<RedeemResponse, ApiError> {
    let body = serde_json::json!({ "member": member });
    let r: RedeemResponse = parse(api("POST", &format!("{server}/v1/rooms/{room}/invites/{invite}/redeem"), None, None, Some(body))?)?;
    // A token is 32 bytes in base64url: anything else would only fail later, in a header, on every reconnect.
    if decode_secret(&r.token).is_err() {
        return Err(ApiError { status: 502, code: "bad_reply".into() });
    }
    Ok(r)
}

pub fn members(server: &str, room: &str, token: &str) -> Result<Vec<MemberInfo>, ApiError> {
    parse(api("GET", &format!("{server}/v1/rooms/{room}/members"), Some(token), None, None)?)
}

pub fn remove_member(server: &str, room: &str, token: &str, member_id: &str) -> Result<(), ApiError> {
    api("DELETE", &format!("{server}/v1/rooms/{room}/members/{member_id}"), Some(token), None, None).map(|_| ())
}

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;
