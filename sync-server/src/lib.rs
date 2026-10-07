//! Lorekeeper sync server: stores encrypted blobs per room and relays them over WebSockets.
//! docs/SYNC.md is the spec. Nothing here logs blobs, tokens, keys, invites or file ids.

mod db;
mod live;

use std::collections::HashMap;
use std::hash::Hash;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::{ConnectInfo, FromRequestParts, Path, Query, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use rusqlite::Connection;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use sync_protocol::{
    decode_blob, decode_secret, encode_blob, encode_secret, is_id, random_id, random_secret, token_hash, Change,
    ChangesResponse, CreateRoomResponse, ErrorResponse, InviteInfo, InviteRequest, MemberUpdate, RedeemRequest,
    RedeemResponse, Role, RoomSettings, CREATE_KEY_HEADER, NONCE_LEN, TAG_LEN,
};
use tokio::sync::watch;

pub use db::now_ms;

const MIB: u64 = 1024 * 1024;
/// Members plus pending invites per room.
const MAX_SEATS: u64 = 32;
/// Longest opaque display id accepted.
const MAX_MEMBER_LEN: usize = 512;
/// Blob bytes per page of changes (always at least one change). It bounds memory per socket: a
/// page is held as base64 and again as its JSON frame while it's sent.
const PAGE_BYTES: usize = 4 * MIB as usize;
/// Socket upgrades plus `GET /changes` per member per minute; each can read the whole room.
const READS_PER_MINUTE: u32 = 60;
/// Most clients a rate limit tracks at once; past it (after dropping stale ones) new clients are
/// refused until a window ends, so a flood of addresses can't grow the map without bound.
const MAX_TRACKED: usize = 10_000;
/// Room deletions plus leaves per client IP per hour.
const DELETES_PER_HOUR: u32 = 20;

#[derive(Clone, Debug)]
pub struct Config {
    pub db_path: PathBuf,
    pub addr: SocketAddr,
    pub public_url: String,
    /// Read the client IP from `CF-Connecting-IP`. Only safe when nothing but cloudflared can
    /// reach the server: anyone else can send the header.
    pub trust_cloudflare: bool,
    pub max_blob: usize,
    pub max_room_bytes: u64,
    pub max_files: u64,
    pub writes_per_minute: u32,
    pub rooms_per_hour: u32,
    pub redeems_per_hour: u32,
    /// Live sockets per room.
    pub max_connections: usize,
    /// Live sockets on the whole server.
    pub max_connections_total: usize,
    pub invite_days: u32,
    /// `POST /v1/rooms` needs it in `X-Lorekeeper-Create-Key`; `None` refuses every creation.
    /// `from_env` refuses to start without one.
    pub create_key: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            db_path: "/data/sync.db".into(),
            addr: ([0, 0, 0, 0], 8080).into(),
            public_url: "https://lorekeeper.yonatankarp.com".into(),
            trust_cloudflare: false,
            max_blob: (30 * MIB) as usize,
            max_room_bytes: 1024 * MIB,
            max_files: 20_000,
            writes_per_minute: 60,
            rooms_per_hour: 5,
            redeems_per_hour: 20,
            max_connections: 32,
            max_connections_total: 64,
            invite_days: 7,
            create_key: None,
        }
    }
}

impl Config {
    /// Reads `LOREKEEPER_SYNC_*` from the environment (see docs/SYNC.md).
    pub fn from_env() -> Result<Config, String> {
        Self::from_vars(|name| std::env::var(name).ok())
    }

    pub fn from_vars(var: impl Fn(&str) -> Option<String>) -> Result<Config, String> {
        fn num<T: std::str::FromStr>(var: &impl Fn(&str) -> Option<String>, name: &str, default: T) -> Result<T, String> {
            match var(name) {
                Some(v) if !v.trim().is_empty() => v.trim().parse().map_err(|_| format!("{name} is invalid")),
                _ => Ok(default),
            }
        }
        let d = Config::default();
        let trust = var("LOREKEEPER_SYNC_TRUST_PROXY").unwrap_or_default();
        let trust_cloudflare = match trust.trim() {
            "cloudflare" => true,
            "none" | "" => false,
            _ => return Err("LOREKEEPER_SYNC_TRUST_PROXY must be cloudflare or none".into()),
        };
        // Opaque: used exactly as given (it may contain / + =), only an empty value means unset.
        // Required: the app is public, so an open server is anyone's disk.
        let create_key = var("LOREKEEPER_SYNC_CREATE_KEY").filter(|k| !k.is_empty());
        if create_key.as_ref().is_none_or(|k| k.chars().count() < 32) {
            return Err("LOREKEEPER_SYNC_CREATE_KEY must be set, at least 32 characters".into());
        }
        Ok(Config {
            db_path: var("LOREKEEPER_SYNC_DB").map_or(d.db_path, PathBuf::from),
            addr: num(&var, "LOREKEEPER_SYNC_ADDR", d.addr)?,
            public_url: var("LOREKEEPER_SYNC_PUBLIC_URL").map_or(d.public_url, |u| u.trim_end_matches('/').to_string()),
            trust_cloudflare,
            max_blob: num(&var, "LOREKEEPER_SYNC_MAX_BLOB", d.max_blob)?,
            max_room_bytes: num(&var, "LOREKEEPER_SYNC_MAX_ROOM_BYTES", d.max_room_bytes)?,
            max_files: num(&var, "LOREKEEPER_SYNC_MAX_FILES", d.max_files)?,
            writes_per_minute: num(&var, "LOREKEEPER_SYNC_WRITES_PER_MINUTE", d.writes_per_minute)?,
            rooms_per_hour: num(&var, "LOREKEEPER_SYNC_ROOMS_PER_HOUR", d.rooms_per_hour)?,
            redeems_per_hour: num(&var, "LOREKEEPER_SYNC_REDEEMS_PER_HOUR", d.redeems_per_hour)?,
            max_connections: num(&var, "LOREKEEPER_SYNC_MAX_CONNECTIONS", d.max_connections)?,
            max_connections_total: num(&var, "LOREKEEPER_SYNC_MAX_CONNECTIONS_TOTAL", d.max_connections_total)?,
            invite_days: num(&var, "LOREKEEPER_SYNC_INVITE_DAYS", d.invite_days)?,
            create_key,
        })
    }

    /// Largest WebSocket message: a blob in base64 inside a JSON frame.
    pub fn max_frame(&self) -> usize {
        self.max_blob.div_ceil(3) * 4 + 64 * 1024
    }
}

/// An error answer: HTTP status plus a short code (`{"error": code}`, or a WebSocket error frame).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiError(pub StatusCode, pub &'static str);

impl ApiError {
    const UNAUTHORIZED: ApiError = ApiError(StatusCode::UNAUTHORIZED, "unauthorized");
    const INTERNAL: ApiError = ApiError(StatusCode::INTERNAL_SERVER_ERROR, "internal");
    const RATE_LIMITED: ApiError = ApiError(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut res = (self.0, Json(ErrorResponse { error: self.1.into() })).into_response();
        if self.0 == StatusCode::UNAUTHORIZED {
            res.headers_mut().insert(header::WWW_AUTHENTICATE, header::HeaderValue::from_static("Bearer"));
        }
        res
    }
}

type Windows<K> = Mutex<HashMap<K, (Instant, u32)>>;

pub(crate) struct Inner {
    config: Config,
    db: Mutex<Connection>,
    hubs: Mutex<live::Hubs>,
    writes: Windows<String>,
    reads: Windows<String>,
    creates: Windows<IpAddr>,
    redeems: Windows<IpAddr>,
    deletes: Windows<IpAddr>,
    next_conn: AtomicU64,
    shutdown: watch::Sender<bool>,
}

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl AppState {
    /// Opens (or creates) the database.
    pub fn new(config: Config) -> Result<AppState, String> {
        let conn = db::open(&config.db_path).map_err(|e| format!("can't open {}: {e}", config.db_path.display()))?;
        Ok(AppState(Arc::new(Inner {
            config,
            db: Mutex::new(conn),
            hubs: Mutex::default(),
            writes: Mutex::default(),
            reads: Mutex::default(),
            creates: Mutex::default(),
            redeems: Mutex::default(),
            deletes: Mutex::default(),
            next_conn: AtomicU64::new(1),
            shutdown: watch::Sender::new(false),
        })))
    }

    /// Tells live sockets to close (graceful shutdown doesn't wait for upgraded connections).
    pub fn shut_down(&self) {
        self.0.shutdown.send_replace(true);
    }

    // ponytail: one SQLite connection behind a global lock; plenty for a party, pool readers if
    // many rooms ever share a server.
    async fn db<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Connection) -> rusqlite::Result<R> + Send + 'static,
    ) -> Result<R, ApiError> {
        let state = self.clone();
        tokio::task::spawn_blocking(move || f(&mut lock(&state.0.db)))
            .await
            .map_err(|_| ApiError::INTERNAL)?
            .map_err(|e| {
                eprintln!("database error: {e}");
                ApiError::INTERNAL
            })
    }
}

// ponytail: fixed windows; a burst at a window edge can reach twice the limit, which is fine here.
fn allow<K: Hash + Eq>(windows: &Windows<K>, key: K, limit: u32, window: Duration) -> bool {
    let mut map = lock(windows);
    let now = Instant::now();
    if map.len() >= MAX_TRACKED && !map.contains_key(&key) {
        map.retain(|_, (start, _)| now.duration_since(*start) < window);
        if map.len() >= MAX_TRACKED {
            return false;
        }
    }
    let entry = map.entry(key).or_insert((now, 0));
    if now.duration_since(entry.0) >= window {
        *entry = (now, 0);
    }
    if entry.1 >= limit {
        return false;
    }
    entry.1 += 1;
    true
}

/// The client, for per-IP rate limits. An IPv6 client is its /64: one host usually has the whole
/// prefix and can pick a new address per request.
fn client_ip(state: &AppState, headers: &HeaderMap, peer: SocketAddr) -> IpAddr {
    let header = || headers.get("cf-connecting-ip")?.to_str().ok()?.trim().parse().ok();
    let ip = state.0.config.trust_cloudflare.then(header).flatten().unwrap_or(peer.ip()).to_canonical();
    match ip {
        IpAddr::V6(v6) => IpAddr::V6((u128::from(v6) & u128::MAX << 64).into()),
        v4 => v4,
    }
}

/// A member's budget of socket upgrades and `GET /changes`.
fn allow_read(state: &AppState, caller: &Caller) -> Result<(), ApiError> {
    let ok = allow(&state.0.reads, caller.member_id.clone(), READS_PER_MINUTE, Duration::from_secs(60));
    ok.then_some(()).ok_or(ApiError::RATE_LIMITED)
}

/// Who is calling: a token of the room. `role` and `manage` are as of the call; what a member may read is decided in
/// the database instead, on every read (db::changes, db::access).
#[derive(Clone, Debug)]
pub(crate) struct Caller {
    room: String,
    member_id: String,
    role: Role,
    manage: bool,
}

impl Caller {
    /// The owner, or a DM the owner lets manage members.
    fn manages(&self) -> bool {
        self.role == Role::Owner || (self.role == Role::Dm && self.manage)
    }
}

/// Checks a token against every token of the room in constant time. Unknown rooms, revoked and
/// malformed tokens all give the same 401.
async fn authenticate(state: &AppState, room: &str, token: Option<&str>) -> Result<Caller, ApiError> {
    let token = token.and_then(|t| decode_secret(t).ok()).ok_or(ApiError::UNAUTHORIZED)?;
    if !is_id(room) {
        return Err(ApiError::UNAUTHORIZED);
    }
    let hash = token_hash(&token);
    let owned = room.to_string();
    let rows = state.db(move |c| db::room_tokens(c, &owned)).await?;
    let mut found = None;
    let dummy = (vec![0u8; 32], String::new(), Role::Player, false);
    for (stored, member_id, role, manage) in rows.iter().chain(rows.is_empty().then_some(&dummy)) {
        if bool::from(stored.as_slice().ct_eq(&hash)) {
            found = Some((member_id.clone(), *role, *manage));
        }
    }
    let (member_id, role, manage) = found.filter(|(id, ..)| !id.is_empty()).ok_or(ApiError::UNAUTHORIZED)?;
    Ok(Caller { room: room.to_string(), member_id, role, manage })
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers.get(header::AUTHORIZATION)?.to_str().ok()?.strip_prefix("Bearer ").map(str::trim)
}

impl FromRequestParts<AppState> for Caller {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let Path(params) = Path::<HashMap<String, String>>::from_request_parts(parts, state)
            .await
            .map_err(|_| ApiError::UNAUTHORIZED)?;
        let room = params.get("room").map_or("", String::as_str);
        authenticate(state, room, bearer(&parts.headers)).await
    }
}

/// A caller holding the room's owner token.
pub(crate) struct Owner(Caller);

impl FromRequestParts<AppState> for Owner {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let caller = Caller::from_request_parts(parts, state).await?;
        if caller.role != Role::Owner {
            return Err(OWNER_ONLY);
        }
        Ok(Owner(caller))
    }
}

/// A caller who manages members: the owner, or a DM with `manage`.
pub(crate) struct Manager(Caller);

impl FromRequestParts<AppState> for Manager {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let caller = Caller::from_request_parts(parts, state).await?;
        if !caller.manages() {
            return Err(ApiError(StatusCode::FORBIDDEN, "not_allowed"));
        }
        Ok(Manager(caller))
    }
}

const OWNER_ONLY: ApiError = ApiError(StatusCode::FORBIDDEN, "owner_only");
const NOT_FOUND: ApiError = ApiError(StatusCode::NOT_FOUND, "not_found");

/// Whether `caller` (who manages) may remove or re-invite `target`: the owner may anyone but themselves, a manager
/// only members who don't manage. The owner as target is refused with `owner_error`.
fn may_handle(caller: &Caller, target: &db::Member, owner_error: &'static str) -> Result<(), ApiError> {
    if target.role == Role::Owner {
        return Err(ApiError(StatusCode::BAD_REQUEST, owner_error));
    }
    if caller.role != Role::Owner && target.manage {
        return Err(OWNER_ONLY);
    }
    Ok(())
}

pub(crate) enum Outcome {
    Ack(u64),
    Conflict(u64, Option<String>),
}

/// The one write path (WebSocket put and delete): rate limit, validation, limits, then the
/// conditional write; wakes the room's live sockets. `owner` is "" for the shared files, else the writer's own
/// member id (their private space): the socket picks it from the writer's token, never from the message.
pub(crate) async fn write(
    state: &AppState,
    room: &str,
    writer: &str,
    owner: &str,
    id: &str,
    base: u64,
    blob: Option<&str>,
) -> Result<Outcome, ApiError> {
    let cfg = &state.0.config;
    if !allow(&state.0.writes, room.to_string(), cfg.writes_per_minute, Duration::from_secs(60)) {
        return Err(ApiError::RATE_LIMITED);
    }
    if !is_id(id) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "bad_id"));
    }
    let blob = match blob {
        None => None,
        Some(b) => {
            if b.len() > cfg.max_blob.div_ceil(3) * 4 {
                return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, "too_large"));
            }
            let bytes = decode_blob(b).map_err(|_| ApiError(StatusCode::BAD_REQUEST, "bad_blob"))?;
            if bytes.len() > cfg.max_blob {
                return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, "too_large"));
            }
            if bytes.len() < NONCE_LEN + TAG_LEN {
                return Err(ApiError(StatusCode::BAD_REQUEST, "bad_blob"));
            }
            Some(bytes)
        }
    };
    let limits = db::Limits { max_room_bytes: cfg.max_room_bytes, max_files: cfg.max_files };
    let (r, w, o, i) = (room.to_string(), writer.to_string(), owner.to_string(), id.to_string());
    let written = state.db(move |c| db::write(c, &r, &w, &o, &i, base, blob.as_deref(), &limits)).await?;
    match written {
        db::Written::Ack(seq) => {
            live::notify(state, room, seq);
            Ok(Outcome::Ack(seq))
        }
        db::Written::Conflict(seq, blob) => Ok(Outcome::Conflict(seq, blob.map(|b| encode_blob(&b)))),
        db::Written::RoomFull => Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, "room_full")),
        db::Written::TooManyFiles => Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, "too_many_files")),
        db::Written::Gone => Err(ApiError::UNAUTHORIZED),
    }
}

/// How many changes a replay from `since` (others' private files from `dm_since`) holds; see [`db::count_changes`].
pub(crate) async fn replay_total(state: &AppState, room: &str, viewer: &str, since: u64, dm_since: u64) -> Result<u64, ApiError> {
    let max = i64::MAX as u64;
    let (r, v, since, dm_since) = (room.to_string(), viewer.to_string(), since.min(max), dm_since.min(max));
    state.db(move |c| db::count_changes(c, &r, &v, since, dm_since)).await
}

/// One page of the changes `viewer` may see after `since` (others' private files after `dm_since`); see
/// [`db::changes`], where visibility is decided.
pub(crate) async fn changes_page(
    state: &AppState,
    room: &str,
    viewer: &str,
    since: u64,
    dm_since: u64,
) -> Result<ChangesResponse, ApiError> {
    // SQLite integers stop at i64::MAX; no seq is ever above it.
    let max = i64::MAX as u64;
    let (r, v, since, dm_since) = (room.to_string(), viewer.to_string(), since.min(max), dm_since.min(max));
    let (seq, rows, more) = state.db(move |c| db::changes(c, &r, &v, since, dm_since, PAGE_BYTES)).await?;
    let changes = rows
        .into_iter()
        .map(|(id, seq, blob, owner)| Change {
            id,
            seq,
            blob: blob.map(|b| encode_blob(&b)),
            author: (!owner.is_empty()).then_some(owner),
        })
        .collect();
    Ok(ChangesResponse { seq, changes, more })
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(|| async { Redirect::temporary("https://yonatankarp.com/lorekeeper/") }))
        .route("/healthz", get(|| async { "ok" }))
        .route("/join/{room}/{invite}", get(join_page))
        .route("/icon.png", get(icon))
        .route("/v1/rooms", post(create_room))
        .route("/v1/rooms/{room}", delete(delete_room))
        .route("/v1/rooms/{room}/changes", get(changes))
        .route("/v1/rooms/{room}/live", get(live::upgrade))
        .route("/v1/rooms/{room}/invites", post(create_invite).get(list_invites))
        .route("/v1/rooms/{room}/invites/{invite}", delete(revoke_invite))
        .route("/v1/rooms/{room}/invites/{invite}/redeem", post(redeem))
        .route("/v1/rooms/{room}/members", get(members))
        .route("/v1/rooms/{room}/members/me", delete(leave_room))
        .route("/v1/rooms/{room}/members/{member_id}", delete(remove_member).patch(update_member))
        .route("/v1/rooms/{room}/members/{member_id}/reinvite", post(reinvite))
        .route("/v1/rooms/{room}/settings", patch(update_settings))
        .layer(axum::extract::DefaultBodyLimit::max(16 * 1024))
        .with_state(state)
}

async fn create_room(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let cfg = &state.0.config;
    // The key first, so only its holders count against (and can fill) the per-IP limit; at 32+
    // characters it needs no rate limit against guessing. Digests, so the compare takes the same
    // time whatever the given key's length.
    let given = headers.get(CREATE_KEY_HEADER).map(|v| Sha256::digest(v.as_bytes()));
    let wanted = cfg.create_key.as_ref().map(|k| Sha256::digest(k.as_bytes()));
    let ok = given.zip(wanted).is_some_and(|(g, w)| bool::from(g.as_slice().ct_eq(w.as_slice())));
    if !ok {
        return Err(ApiError(StatusCode::FORBIDDEN, "create_key_required"));
    }
    let ip = client_ip(&state, &headers, peer);
    if !allow(&state.0.creates, ip, cfg.rooms_per_hour, Duration::from_secs(3600)) {
        return Err(ApiError::RATE_LIMITED);
    }
    let (room, token, owner_id) = (random_id(), random_secret(), random_id());
    let (r, hash) = (room.clone(), token_hash(&token));
    state.db(move |c| db::create_room(c, &r, &hash, &owner_id)).await?;
    Ok((StatusCode::CREATED, Json(CreateRoomResponse { room, owner_token: encode_secret(&token) })).into_response())
}

#[derive(Deserialize)]
struct Since {
    #[serde(default)]
    since: u64,
}

async fn changes(
    State(state): State<AppState>,
    caller: Caller,
    query: Result<Query<Since>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<ChangesResponse>, ApiError> {
    let Query(Since { since }) = query.map_err(|_| ApiError(StatusCode::BAD_REQUEST, "bad_request"))?;
    allow_read(&state, &caller)?;
    Ok(Json(changes_page(&state, &caller.room, &caller.member_id, since, since).await?))
}

/// A JSON body, or the default for an empty one.
fn body_or_default<T: serde::de::DeserializeOwned + Default>(body: &Bytes) -> Result<T, ApiError> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice(body).map_err(|_| ApiError(StatusCode::BAD_REQUEST, "bad_request"))
}

/// Stores a new invite (`member_id` None, if the room has a seat for one) or a re-invite.
async fn add_invite(state: &AppState, room: String, mut info: InviteInfo) -> Result<Response, ApiError> {
    info.expires = now_ms() + i64::from(state.0.config.invite_days) * 86_400_000;
    let stored = info.clone();
    state
        .db(move |c| {
            if stored.member_id.is_none() && db::seats(c, &room)? >= MAX_SEATS {
                return Ok(Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, "too_many_members")));
            }
            // A room deleted since the token was checked answers as deleted rooms do.
            Ok(if db::create_invite(c, &room, &stored)? { Ok(()) } else { Err(ApiError::UNAUTHORIZED) })
        })
        .await??;
    Ok((StatusCode::CREATED, Json(info)).into_response())
}

/// Invites a player or a DM. Managers can invite both, but only the owner can let a DM manage.
async fn create_invite(State(state): State<AppState>, Manager(caller): Manager, body: Bytes) -> Result<Response, ApiError> {
    let req: InviteRequest = body_or_default(&body)?;
    if req.role == Role::Owner || (req.manage && req.role != Role::Dm) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "bad_request"));
    }
    if req.manage && caller.role != Role::Owner {
        return Err(OWNER_ONLY);
    }
    let info = InviteInfo { invite: random_id(), expires: 0, role: req.role, manage: req.manage, member_id: None };
    add_invite(&state, caller.room, info).await
}

/// An invite that hands an existing member's identity to whoever redeems it (their new computer): see db::redeem.
async fn reinvite(
    State(state): State<AppState>,
    Manager(caller): Manager,
    Path((_, member_id)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    if !is_id(&member_id) {
        return Err(NOT_FOUND);
    }
    let (room, id) = (caller.room.clone(), member_id.clone());
    let target = state.db(move |c| db::member(c, &room, &id)).await?.ok_or(NOT_FOUND)?;
    may_handle(&caller, &target, "cannot_reinvite_owner")?;
    let info = InviteInfo { invite: random_id(), expires: 0, role: target.role, manage: target.manage, member_id: Some(member_id) };
    add_invite(&state, caller.room, info).await
}

/// Pending invites. A manager doesn't see one that grants manage or re-invites someone who manages: they hold the room
/// key, so an invite they can see is one they could redeem themselves.
async fn list_invites(State(state): State<AppState>, Manager(caller): Manager) -> Result<Json<Vec<InviteInfo>>, ApiError> {
    let owner = caller.role == Role::Owner;
    let list = state
        .db(move |c| {
            let mut out = Vec::new();
            for i in db::pending_invites(c, &caller.room)? {
                if owner || !(i.manage || invite_target_manages(c, &caller.room, &i.member_id)?) {
                    out.push(i);
                }
            }
            Ok(out)
        })
        .await?;
    Ok(Json(list))
}

/// Whether a re-invite's member manages (false for an invite for someone new).
fn invite_target_manages(c: &Connection, room: &str, target: &Option<String>) -> rusqlite::Result<bool> {
    match target {
        Some(id) => Ok(db::member(c, room, id)?.is_some_and(|m| m.manage)),
        None => Ok(false),
    }
}

/// Cancels a pending invite. A manager can't cancel one that grants manage or re-invites someone who manages.
async fn revoke_invite(
    State(state): State<AppState>,
    Manager(caller): Manager,
    Path((_, invite)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let owner = caller.role == Role::Owner;
    state
        .db(move |c| {
            let Some((_, manage, target)) = db::invite(c, &caller.room, &invite)? else {
                return Ok(Err(ApiError(StatusCode::NOT_FOUND, "invite_not_found")));
            };
            if !owner && (manage || invite_target_manages(c, &caller.room, &target)?) {
                return Ok(Err(OWNER_ONLY));
            }
            db::revoke_invite(c, &caller.room, &invite)?;
            Ok(Ok(()))
        })
        .await??;
    Ok(StatusCode::NO_CONTENT)
}

async fn redeem(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path((room, invite)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let ip = client_ip(&state, &headers, peer);
    if !allow(&state.0.redeems, ip, state.0.config.redeems_per_hour, Duration::from_secs(3600)) {
        return Err(ApiError::RATE_LIMITED);
    }
    let req: RedeemRequest =
        serde_json::from_slice(&body).map_err(|_| ApiError(StatusCode::BAD_REQUEST, "bad_request"))?;
    if req.member.len() > MAX_MEMBER_LEN {
        return Err(ApiError(StatusCode::BAD_REQUEST, "member_too_long"));
    }
    let not_found = ApiError(StatusCode::NOT_FOUND, "invite_not_found");
    if !is_id(&room) || !is_id(&invite) {
        return Err(not_found);
    }
    let token = random_secret();
    let hash = token_hash(&token);
    let member_id = random_id();
    let r = room.clone();
    let result = state.db(move |c| db::redeem(c, &r, &invite, &hash, &member_id, &req.member)).await?;
    let created = || (StatusCode::CREATED, Json(RedeemResponse { token: encode_secret(&token) })).into_response();
    match result {
        db::Redeemed::Joined => {
            live::access_changed(&state, &room); // DMs who read private notes learn of the new member
            Ok(created())
        }
        db::Redeemed::Replaced(member_id) => {
            live::kick(&state, &room, Some(&member_id), live::REPLACED); // the member's old computer is signed out at once
            Ok(created())
        }
        db::Redeemed::NotFound => Err(not_found),
        db::Redeemed::Used => Err(ApiError(StatusCode::GONE, "invite_used")),
        db::Redeemed::Expired => Err(ApiError(StatusCode::GONE, "invite_expired")),
    }
}

async fn members(
    State(state): State<AppState>,
    Manager(caller): Manager,
) -> Result<Json<Vec<sync_protocol::MemberInfo>>, ApiError> {
    Ok(Json(state.db(move |c| db::members(c, &caller.room)).await?))
}

/// Removes a member and deletes their private notes from the server. A manager may remove members who don't manage.
async fn remove_member(
    State(state): State<AppState>,
    Manager(caller): Manager,
    Path((_, member_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let (room, id, by) = (caller.room.clone(), member_id.clone(), caller.clone());
    state
        .db(move |c| {
            let Some(target) = db::member(c, &room, &id)? else { return Ok(Err(NOT_FOUND)) };
            if let Err(e) = may_handle(&by, &target, "cannot_remove_owner") {
                return Ok(Err(e));
            }
            db::remove_member(c, &room, &id)?;
            Ok(Ok(()))
        })
        .await??;
    live::kick(&state, &caller.room, Some(&member_id), live::REVOKED);
    live::access_changed(&state, &caller.room); // DMs drop their copies of the member's private notes
    Ok(StatusCode::NO_CONTENT)
}

/// The per-IP budget of room deletions and leaves; checked after the token, so only a room's members count against it.
fn allow_delete(state: &AppState, headers: &HeaderMap, peer: SocketAddr) -> Result<(), ApiError> {
    let ip = client_ip(state, headers, peer);
    let ok = allow(&state.0.deletes, ip, DELETES_PER_HOUR, Duration::from_secs(3600));
    ok.then_some(()).ok_or(ApiError::RATE_LIMITED)
}

/// A member leaves the room on their own: like a removal (their token stops working, their sockets close with 4001,
/// their private space is deleted), asked by the member. The owner can't leave; they delete the room instead.
async fn leave_room(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    caller: Caller,
) -> Result<StatusCode, ApiError> {
    if caller.role == Role::Owner {
        return Err(ApiError(StatusCode::BAD_REQUEST, "owner_cannot_leave"));
    }
    allow_delete(&state, &headers, peer)?;
    let (room, id) = (caller.room.clone(), caller.member_id.clone());
    state.db(move |c| db::remove_member(c, &room, &id)).await?;
    live::kick(&state, &caller.room, Some(&caller.member_id), live::REVOKED);
    live::access_changed(&state, &caller.room);
    Ok(StatusCode::NO_CONTENT)
}

/// The owner deletes the room: every file (shared and private), member, token and invite goes in one transaction, then
/// every socket closes with 4003. Deleting first means an upgrade racing it finds no token (touch_token) and gets 401.
async fn delete_room(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Owner(caller): Owner,
) -> Result<StatusCode, ApiError> {
    allow_delete(&state, &headers, peer)?;
    let room = caller.room.clone();
    state.db(move |c| db::delete_room(c, &room)).await?;
    live::kick(&state, &caller.room, None, live::DELETED);
    Ok(StatusCode::NO_CONTENT)
}

/// The owner changes a member's role or a DM's `manage` flag, or says on their own row that they're also the DM; the
/// member's sockets learn it at once.
async fn update_member(
    State(state): State<AppState>,
    Owner(caller): Owner,
    Path((_, member_id)): Path<(String, String)>,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    let req: MemberUpdate = serde_json::from_slice(&body).map_err(|_| ApiError(StatusCode::BAD_REQUEST, "bad_request"))?;
    let room = caller.room.clone();
    match state.db(move |c| db::update_member(c, &room, &member_id, &req)).await? {
        db::Updated::Yes => {
            live::access_changed(&state, &caller.room);
            Ok(StatusCode::NO_CONTENT)
        }
        db::Updated::Owner => Err(ApiError(StatusCode::BAD_REQUEST, "cannot_change_owner")),
        db::Updated::NotFound => Err(NOT_FOUND),
        db::Updated::Invalid => Err(ApiError(StatusCode::BAD_REQUEST, "bad_request")),
    }
}

/// The owner turns `dm_reads_private` on or off. Every socket learns it, and DMs start or stop receiving private notes
/// at once (db::changes reads the setting for every page it sends).
async fn update_settings(State(state): State<AppState>, Owner(caller): Owner, body: Bytes) -> Result<Json<RoomSettings>, ApiError> {
    let req: RoomSettings = serde_json::from_slice(&body).map_err(|_| ApiError(StatusCode::BAD_REQUEST, "bad_request"))?;
    let room = caller.room.clone();
    state.db(move |c| db::set_dm_reads_private(c, &room, req.dm_reads_private)).await?;
    live::access_changed(&state, &caller.room);
    Ok(Json(req))
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// What a browser shows for an invite link. It never touches the invite, loads only the app's icon from this server
/// and can't be framed. Its one
/// script (allowed by its hash, nothing else runs) hands the link to the app as `lorekeeper://join?link=<the link>`,
/// so the browser asks "Open Lorekeeper?", and puts it on the **Open in Lorekeeper** button too. A browser without
/// Lorekeeper 0.7+ may show an error for that; the page says what's needed. The key in the fragment goes only there:
/// no request, no log, no storage.
async fn join_page(State(state): State<AppState>) -> impl IntoResponse {
    let headers = [
        (header::CONTENT_SECURITY_POLICY, JOIN_CSP.as_str()),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::REFERRER_POLICY, "no-referrer"),
    ];
    let page = JOIN_PAGE.replace("{script}", JOIN_SCRIPT).replace("{server}", &escape(&state.0.config.public_url));
    (headers, Html(page))
}

/// The app's icon, for the join page: built in, so the page loads nothing from anywhere else.
async fn icon() -> impl IntoResponse {
    let headers = [
        (header::CONTENT_TYPE, "image/png"),
        (header::CACHE_CONTROL, "public, max-age=86400"),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
    ];
    (headers, &include_bytes!("../assets/icon.png")[..])
}

/// The join page's policy: its own script by hash, inline styles, the icon from this server, nothing else.
static JOIN_CSP: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    let hash = encode_blob(&Sha256::digest(JOIN_SCRIPT.as_bytes()));
    format!("default-src 'none'; script-src 'sha256-{hash}'; style-src 'unsafe-inline'; img-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'")
});

/// Runs only for a well-formed link: the path the server routed here and a 32-byte key after #. It fills in and shows
/// the button, and navigates to the app's link and nowhere else.
const JOIN_SCRIPT: &str = r#"(function () {
  var path = location.pathname, key = location.hash;
  if (!/^\/join\/[a-z2-7]{26}\/[a-z2-7]{26}$/.test(path) || !/^#[A-Za-z0-9_-]{43}$/.test(key)) return;
  var url = "lorekeeper://join?link=" + encodeURIComponent(location.origin + path + key);
  var open = document.getElementById("open");
  open.href = url;
  open.hidden = false;
  location.replace(url);
})();"#;

const JOIN_PAGE: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="referrer" content="no-referrer">
<title>Lorekeeper invite</title>
<link rel="icon" href="/icon.png">
<style>
  :root { color-scheme: light dark; --bg: #f4ecd8; --card: #fbf6e9; --ink: #3b2f22; --muted: #7a6650; --accent: #8b2e1f; --line: #d9c9a3; --on-accent: #fbf6e9; }
  @media (prefers-color-scheme: dark) { :root { --bg: #1f1a14; --card: #2a231b; --ink: #ece2cc; --muted: #b3a284; --accent: #e07a5f; --line: #4a3f30; --on-accent: #1f1a14; } }
  body { margin: 0; min-height: 100vh; display: grid; place-items: center; background: var(--bg); color: var(--ink);
         font: 17px/1.55 Georgia, "Iowan Old Style", "Palatino Linotype", serif; }
  main { max-width: 32rem; margin: 16px; padding: 28px 30px; background: var(--card); border: 1px solid var(--line); border-radius: 10px; }
  .logo { display: block; margin: 0 0 14px; }
  h1 { margin: 0 0 12px; font-size: 1.5rem; color: var(--accent); font-weight: normal; }
  p { margin: 0 0 12px; }
  b { font-weight: 600; }
  a { color: var(--accent); }
  .open { display: inline-block; padding: 8px 18px; border-radius: 8px; background: var(--accent); color: var(--on-accent); text-decoration: none; }
  [hidden] { display: none !important; }
  small { color: var(--muted); }
</style>
</head>
<body>
<main>
  <img class="logo" src="/icon.png" width="64" height="64" alt="Lorekeeper">
  <h1>You've been invited to a shared campaign</h1>
  <p><a id="open" class="open" hidden>Open in Lorekeeper</a></p>
  <p>Your browser may ask to open Lorekeeper. If nothing happens, click <b>Open in Lorekeeper</b>. Lorekeeper then shows the invite, and joins only when you click <b>Join</b>.</p>
  <p>Needs Lorekeeper 0.7 or later. Don't have it, or have an older version? <a href="https://yonatankarp.com/lorekeeper/" rel="noreferrer">Download Lorekeeper</a>, then open this link again.</p>
  <p>Or open the invite in Lorekeeper yourself: <b>Settings &gt; General &gt; Join a shared campaign</b>, then paste the whole link, including the part after <b>#</b>.</p>
  <p>The link holds the campaign's key. Keep it within your party.</p>
  <small>Lorekeeper sync server at {server}. It stores only encrypted notes it can't read.</small>
</main>
<script>{script}</script>
</body>
</html>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_map_stays_bounded() {
        let windows: Windows<u32> = Mutex::default();
        let hour = Duration::from_secs(3600);
        for k in 0..MAX_TRACKED as u32 {
            assert!(allow(&windows, k, 2, hour));
        }
        assert!(!allow(&windows, u32::MAX, 2, hour), "a new client past the cap is refused");
        assert!(allow(&windows, 7, 2, hour), "known clients keep their budget");
        assert!(!allow(&windows, 7, 2, hour));
        assert_eq!(lock(&windows).len(), MAX_TRACKED);
        // Once windows end, stale clients make room.
        assert!(allow(&windows, u32::MAX, 2, Duration::ZERO));
    }
}
