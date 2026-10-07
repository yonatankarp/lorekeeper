//! `GET /v1/rooms/{room}/live`: the sync channel. Each room has a hub with the latest `seq`
//! (a watch: sockets wake on writes and read what's new from the database, so nothing is missed
//! or reordered), a counter bumped whenever anyone's access changes, and the connected members
//! (presence). What a socket may read is decided by the database on every read (db::changes).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use sync_protocol::{
    decode_secret, token_hash, ClientMessage, PresenceMember, Role, ServerMessage, Space, WS_PROTOCOL, WS_TOKEN_PROTOCOL_PREFIX,
};
use tokio::sync::{watch, Notify};
use tokio::time::{interval_at, timeout, Instant, Interval};

use crate::{
    allow_read, authenticate, bearer, changes_page, db, lock, write, ApiError, AppState, Caller, Outcome, MAX_MEMBER_LEN,
};

const PING_EVERY: Duration = Duration::from_secs(30);
/// Close a socket that sent nothing (not even a pong) for this long.
const IDLE_TIMEOUT: Duration = Duration::from_secs(75);
/// Close a socket that doesn't take a frame (at most a blob in base64) in this long: a reader
/// that stalls must not pin its slot, or the server's memory, for ever.
const SEND_TIMEOUT: Duration = Duration::from_secs(300);
/// A close frame is tiny; a peer that can't take it in this long is gone.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
/// Client messages per socket per minute; more closes the socket (1008). Writes have their own,
/// lower, per-room limit.
const MESSAGES_PER_MINUTE: u32 = 600;
/// Live sockets per member (their devices), so one token can't take a room's or the server's
/// whole share.
const MEMBER_CONNECTIONS: usize = 8;
/// Close codes: the member was removed, or re-invited and signed in on another computer.
pub(crate) const REVOKED: u16 = 4001;
pub(crate) const REPLACED: u16 = 4002;

/// Rooms' hubs, and the number of sockets in all of them (for the server-wide cap).
#[derive(Default)]
pub(crate) struct Hubs(HashMap<String, Hub>, usize);

struct Hub {
    seq: watch::Sender<u64>,
    /// Bumped when a role, a manage flag, the room's setting or the members change: each socket reads its access again.
    access: watch::Sender<u64>,
    presence: watch::Sender<Vec<PresenceMember>>,
    conns: HashMap<u64, Conn>,
}

/// Closes a socket with a code.
#[derive(Default)]
struct Kick {
    notify: Notify,
    code: AtomicU16,
}

struct Conn {
    member_id: String,
    role: Role,
    /// Set by `hello`; until then the connection isn't in presence.
    member: Option<String>,
    kick: Arc<Kick>,
}

impl Hub {
    fn update_presence(&self) {
        // One entry per member, even with several devices connected.
        let members: BTreeMap<&str, PresenceMember> = self
            .conns
            .values()
            .filter_map(|c| {
                let member = c.member.clone()?;
                Some((c.member_id.as_str(), PresenceMember { member_id: c.member_id.clone(), member, role: c.role }))
            })
            .collect();
        let members: Vec<PresenceMember> = members.into_values().collect();
        self.presence.send_if_modified(|p| {
            let changed = *p != members;
            if changed {
                *p = members;
            }
            changed
        });
    }
}

/// Wakes the room's sockets after a write.
pub(crate) fn notify(state: &AppState, room: &str, seq: u64) {
    if let Some(hub) = lock(&state.0.hubs).0.get(room) {
        hub.seq.send_replace(seq);
    }
}

/// Someone's access changed: every socket of the room reads its own again and is told when it differs.
pub(crate) fn access_changed(state: &AppState, room: &str) {
    if let Some(hub) = lock(&state.0.hubs).0.get(room) {
        hub.access.send_modify(|n| *n += 1);
    }
}

/// Closes a member's sockets with `code` (removed, or signed in elsewhere).
pub(crate) fn kick(state: &AppState, room: &str, member_id: &str, code: u16) {
    if let Some(hub) = lock(&state.0.hubs).0.get(room) {
        for c in hub.conns.values().filter(|c| c.member_id == member_id) {
            c.kick.code.store(code, Ordering::Relaxed);
            c.kick.notify.notify_one();
        }
    }
}

/// A socket's place in its room's hub; leaving (drop) updates presence.
struct Slot {
    state: AppState,
    room: String,
    conn: u64,
    seq: watch::Receiver<u64>,
    access: watch::Receiver<u64>,
    presence: watch::Receiver<Vec<PresenceMember>>,
    kick: Arc<Kick>,
}

impl Slot {
    fn join(state: &AppState, caller: &Caller) -> Result<Slot, ApiError> {
        let mut guard = lock(&state.0.hubs);
        let Hubs(rooms, total) = &mut *guard;
        let full = ApiError(StatusCode::TOO_MANY_REQUESTS, "too_many_connections");
        if *total >= state.0.config.max_connections_total {
            return Err(full);
        }
        let hub = rooms.entry(caller.room.clone()).or_insert_with(|| Hub {
            seq: watch::Sender::new(0),
            access: watch::Sender::new(0),
            presence: watch::Sender::new(Vec::new()),
            conns: HashMap::new(),
        });
        let mine = hub.conns.values().filter(|c| c.member_id == caller.member_id).count();
        if hub.conns.len() >= state.0.config.max_connections || mine >= MEMBER_CONNECTIONS {
            return Err(full);
        }
        *total += 1;
        let conn = state.0.next_conn.fetch_add(1, Ordering::Relaxed);
        let kick = Arc::new(Kick::default());
        hub.conns.insert(
            conn,
            Conn { member_id: caller.member_id.clone(), role: caller.role, member: None, kick: kick.clone() },
        );
        Ok(Slot {
            state: state.clone(),
            room: caller.room.clone(),
            conn,
            seq: hub.seq.subscribe(),
            access: hub.access.subscribe(),
            presence: hub.presence.subscribe(),
            kick,
        })
    }

    /// Updates this connection's entry in presence (display id from hello, role from access).
    fn set(&self, member: Option<String>, role: Role) {
        if let Some(hub) = lock(&self.state.0.hubs).0.get_mut(&self.room) {
            if let Some(c) = hub.conns.get_mut(&self.conn) {
                if member.is_some() {
                    c.member = member;
                }
                c.role = role;
            }
            hub.update_presence();
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut hubs = lock(&self.state.0.hubs);
        hubs.1 -= 1;
        if let Some(hub) = hubs.0.get_mut(&self.room) {
            hub.conns.remove(&self.conn);
            if hub.conns.is_empty() {
                hubs.0.remove(&self.room);
            } else {
                hub.update_presence();
            }
        }
    }
}

/// The token from `Authorization: Bearer` (preferred), else the `token.<token>` subprotocol, for
/// clients that can't set headers on an upgrade. Never from the URL, which proxies log.
fn live_token(headers: &HeaderMap) -> Option<&str> {
    bearer(headers).or_else(|| {
        headers
            .get_all("sec-websocket-protocol")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .find_map(|p| p.trim().strip_prefix(WS_TOKEN_PROTOCOL_PREFIX))
    })
}

pub(crate) async fn upgrade(
    State(state): State<AppState>,
    Path(room): Path<String>,
    headers: HeaderMap,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Result<Response, ApiError> {
    // Auth before looking at the upgrade, so a bad token is a 401 like everywhere else.
    let caller = authenticate(&state, &room, live_token(&headers)).await?;
    allow_read(&state, &caller)?;
    let ws = match ws {
        Ok(ws) => ws,
        Err(rejection) => return Ok(rejection.into_response()),
    };
    let slot = Slot::join(&state, &caller)?;
    // After joining the hub: a removal or re-invite that lands before this finds no token (401 here), one that lands
    // after finds the connection to kick. By the token, not the member id, which a re-invite hands to a new token.
    let hash = live_token(&headers).and_then(|t| decode_secret(t).ok()).map(|t| token_hash(&t)).ok_or(ApiError::UNAUTHORIZED)?;
    let r = caller.room.clone();
    if !state.db(move |c| db::touch_token(c, &r, &hash)).await? {
        return Err(ApiError::UNAUTHORIZED);
    }
    let max = state.0.config.max_frame();
    Ok(ws
        .protocols([WS_PROTOCOL])
        .max_message_size(max)
        .max_frame_size(max)
        .on_upgrade(move |socket| run(socket, slot, caller)))
}

/// False when the socket is gone or didn't take the frame within [`SEND_TIMEOUT`].
async fn send(socket: &mut WebSocket, msg: &ServerMessage) -> bool {
    let text = serde_json::to_string(msg).expect("ServerMessage serializes");
    matches!(timeout(SEND_TIMEOUT, socket.send(Message::Text(text.into()))).await, Ok(Ok(())))
}

async fn close(socket: &mut WebSocket, code: u16, reason: &'static str) {
    let frame = Message::Close(Some(CloseFrame { code, reason: reason.into() }));
    let _ = timeout(CLOSE_TIMEOUT, socket.send(frame)).await;
}

struct Session {
    caller: Caller,
    hello: bool,
    /// Highest seq this socket has sent or skipped.
    last_sent: u64,
    /// Seqs of this socket's own writes, not echoed back to it.
    own: HashSet<u64>,
    /// The access last told to the client.
    access: Option<db::Access>,
    last_heard: Instant,
    /// Start of the current minute and the client messages in it.
    window: (Instant, u32),
}

async fn run(mut socket: WebSocket, mut slot: Slot, caller: Caller) {
    let state = slot.state.clone();
    let mut shutdown = state.0.shutdown.subscribe();
    let kick = slot.kick.clone();
    let mut ping = interval_at(Instant::now() + PING_EVERY, PING_EVERY);
    let now = Instant::now();
    let mut s = Session { caller, hello: false, last_sent: 0, own: HashSet::new(), access: None, last_heard: now, window: (now, 0) };
    loop {
        // Removal and shutdown win, and interrupt whatever the step is doing (a long replay, a send
        // to a slow reader), so a removed member gets nothing more.
        let keep = tokio::select! {
            biased;
            _ = kick.notify.notified() => {
                let code = kick.code.load(Ordering::Relaxed);
                close(&mut socket, code, if code == REPLACED { "replaced" } else { "revoked" }).await;
                break;
            }
            _ = async { shutdown.wait_for(|down| *down).await.map(|_| ()) } => {
                close(&mut socket, 1012, "restarting").await;
                break;
            }
            keep = step(&state, &mut socket, &mut slot, &mut s, &mut ping) => keep,
        };
        if !keep {
            break;
        }
    }
}

/// Waits for the next thing to do on the socket and does it; false ends the connection.
async fn step(state: &AppState, socket: &mut WebSocket, slot: &mut Slot, s: &mut Session, ping: &mut Interval) -> bool {
    tokio::select! {
        msg = socket.recv() => {
            let Some(Ok(msg)) = msg else { return false };
            let now = Instant::now();
            s.last_heard = now;
            if now.duration_since(s.window.0) >= Duration::from_secs(60) {
                s.window = (now, 0);
            }
            s.window.1 += 1;
            if s.window.1 > MESSAGES_PER_MINUTE {
                close(socket, 1008, "rate_limited").await;
                return false;
            }
            match msg {
                Message::Text(text) => match serde_json::from_str::<ClientMessage>(text.as_str()) {
                    Ok(m) => handle(state, socket, slot, s, m).await,
                    Err(_) => send(socket, &error(None, "bad_message")).await,
                },
                Message::Binary(_) => send(socket, &error(None, "bad_message")).await,
                Message::Close(_) => false,
                Message::Ping(_) | Message::Pong(_) => true,
            }
        }
        changed = slot.seq.changed(), if s.hello => {
            let last = s.last_sent;
            changed.is_ok() && send_new(state, socket, s, last).await
        }
        changed = slot.access.changed(), if s.hello => changed.is_ok() && refresh_access(state, socket, slot, s).await,
        changed = slot.presence.changed(), if s.hello => {
            let members = slot.presence.borrow_and_update().clone();
            changed.is_ok() && send(socket, &ServerMessage::Presence { members }).await
        }
        _ = ping.tick() => {
            s.last_heard.elapsed() <= IDLE_TIMEOUT
                && matches!(timeout(SEND_TIMEOUT, socket.send(Message::Ping(Default::default()))).await, Ok(Ok(())))
        }
    }
}

fn error(req: Option<u64>, code: &str) -> ServerMessage {
    ServerMessage::Error { req, error: code.into() }
}

fn reads(access: &Option<db::Access>) -> bool {
    access.as_ref().is_some_and(|a| a.role.reads_private(a.owner_is_dm, a.dm_reads_private))
}

/// Reads this member's access from the database; tells the client when it changed. When it now reads others' private
/// notes and didn't before, it gets all of them (they were never sent: its replay and stream left them out). False
/// ends the connection (the member is gone: a kick is on its way).
async fn refresh_access(state: &AppState, socket: &mut WebSocket, slot: &mut Slot, s: &mut Session) -> bool {
    slot.access.borrow_and_update();
    let (r, id) = (s.caller.room.clone(), s.caller.member_id.clone());
    let Ok(Some(access)) = state.db(move |c| db::access(c, &r, &id)).await else { return false };
    if s.access.as_ref() == Some(&access) {
        return true;
    }
    let gained = !reads(&s.access) && access.role.reads_private(access.owner_is_dm, access.dm_reads_private);
    slot.set(None, access.role);
    if !send(socket, &access_message(&s.caller, &access)).await {
        return false;
    }
    s.access = Some(access);
    !gained || send_new(state, socket, s, 0).await
}

fn access_message(caller: &Caller, a: &db::Access) -> ServerMessage {
    ServerMessage::Access {
        member_id: caller.member_id.clone(),
        role: a.role,
        manage: a.manage,
        owner_is_dm: a.owner_is_dm,
        dm_reads_private: a.dm_reads_private,
        members: a.members.clone(),
    }
}

/// Handles one client message; false ends the connection.
async fn handle(state: &AppState, socket: &mut WebSocket, slot: &mut Slot, s: &mut Session, msg: ClientMessage) -> bool {
    match msg {
        ClientMessage::Ping => send(socket, &ServerMessage::Pong).await,
        ClientMessage::Hello { since, member, dm_since } => {
            if s.hello {
                return send(socket, &error(None, "hello_twice")).await;
            }
            if member.len() > MAX_MEMBER_LEN {
                return send(socket, &error(None, "member_too_long")).await;
            }
            // Before reading: a write or an access change from here on wakes the socket again.
            slot.seq.borrow_and_update();
            slot.access.borrow_and_update();
            let (r, id, m) = (s.caller.room.clone(), s.caller.member_id.clone(), member.clone());
            let access = state.db(move |c| {
                db::touch_member(c, &r, &id, &m)?;
                db::access(c, &r, &id)
            });
            let access = match access.await {
                Ok(Some(a)) => a,
                Ok(None) => return false, // removed meanwhile
                Err(_) => return send(socket, &error(None, "internal")).await,
            };
            s.hello = true;
            s.last_sent = since;
            // Who you are first (the client needs its member id for the private files in the replay), then the
            // replay, then presence; writes during the replay wake `seq` again, access changes wake `access`.
            if !send(socket, &access_message(&s.caller, &access)).await {
                return false;
            }
            slot.set(Some(member), access.role);
            s.access = Some(access);
            let (mut since, mut dm_since) = (since, dm_since.unwrap_or(since));
            loop {
                let Ok(page) = changes_page(state, &s.caller.room, &s.caller.member_id, since, dm_since).await else {
                    return send(socket, &error(None, "internal")).await;
                };
                if let Some(last) = page.changes.last() {
                    (since, dm_since) = (since.max(last.seq), dm_since.max(last.seq));
                    s.last_sent = s.last_sent.max(last.seq);
                }
                let more = page.more;
                if !send(socket, &ServerMessage::Changes { seq: page.seq, changes: page.changes, more }).await {
                    return false;
                }
                if !more {
                    break;
                }
            }
            let members = slot.presence.borrow_and_update().clone();
            send(socket, &ServerMessage::Presence { members }).await
        }
        ClientMessage::Put { req, id, base, blob, space } => {
            if !s.hello {
                return send(socket, &error(Some(req), "hello_first")).await;
            }
            let result = write(state, &s.caller.room, &s.caller.member_id, owner(s, space), &id, base, Some(&blob)).await;
            answer(socket, s, req, result).await
        }
        ClientMessage::Delete { req, id, base, space } => {
            if !s.hello {
                return send(socket, &error(Some(req), "hello_first")).await;
            }
            let result = write(state, &s.caller.room, &s.caller.member_id, owner(s, space), &id, base, None).await;
            answer(socket, s, req, result).await
        }
    }
}

/// The space a write goes to: the shared files, or the writer's own private space. Nothing names another member's.
fn owner(s: &Session, space: Space) -> &str {
    match space {
        Space::Shared => "",
        Space::Private => &s.caller.member_id,
    }
}

async fn answer(socket: &mut WebSocket, s: &mut Session, req: u64, result: Result<Outcome, ApiError>) -> bool {
    let msg = match result {
        Ok(Outcome::Ack(seq)) => {
            if seq > s.last_sent {
                s.own.insert(seq);
            }
            ServerMessage::Ack { req, seq }
        }
        Ok(Outcome::Conflict(seq, blob)) => ServerMessage::Conflict { req, seq, blob },
        Err(ApiError(_, code)) => error(Some(req), code),
    };
    send(socket, &msg).await
}

/// Sends every change this socket may see after `last_sent` (others' private files after `dm_since`) except its own
/// writes.
async fn send_new(state: &AppState, socket: &mut WebSocket, s: &mut Session, mut dm_since: u64) -> bool {
    let mut since = s.last_sent;
    loop {
        let Ok(page) = changes_page(state, &s.caller.room, &s.caller.member_id, since, dm_since).await else {
            return send(socket, &error(None, "internal")).await;
        };
        for change in page.changes {
            let seq = change.seq;
            (since, dm_since) = (since.max(seq), dm_since.max(seq));
            s.last_sent = s.last_sent.max(seq);
            if !s.own.remove(&seq) && !send(socket, &ServerMessage::Change(change)).await {
                return false;
            }
        }
        if !page.more {
            break;
        }
    }
    let last = s.last_sent;
    s.own.retain(|seq| *seq > last);
    true
}
