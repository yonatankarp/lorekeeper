//! Starts the server in-process against a temporary database and talks to it over HTTP (tower
//! oneshot) and WebSockets (tokio-tungstenite on a real listener).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, StatusCode};
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sync_protocol::*;
use sync_server::{router, AppState, Config};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tower::ServiceExt;

const KEY: [u8; 32] = [9u8; 32];
const CREATE_KEY: &str = "k/+=k/+=k/+=k/+=k/+=k/+=k/+=k/+=k/+=";

struct Server {
    state: AppState,
    addr: SocketAddr,
    db: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.state.shut_down();
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.db.display()));
        }
    }
}

async fn start(tweak: impl FnOnce(&mut Config)) -> Server {
    let db = std::env::temp_dir().join(format!("lorekeeper-sync-test-{}.db", random_id()));
    let mut config = Config { db_path: db.clone(), addr: ([127, 0, 0, 1], 0).into(), ..Config::default() };
    tweak(&mut config);
    let state = AppState::new(config).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router(state.clone()).into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Server { state, addr, db }
}

impl Server {
    async fn http(&self, method: &str, uri: &str, token: Option<&str>, headers: &[(&str, &str)], body: Option<Value>) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let body = body.map_or(Body::empty(), |b| Body::from(b.to_string()));
        let app = router(self.state.clone()).layer(MockConnectInfo(SocketAddr::from(([10, 0, 0, 1], 1234))));
        let res = app.oneshot(req.body(body).unwrap()).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into())))
    }

    /// A new room: (room, owner token).
    async fn room(&self) -> (String, String) {
        let (status, body) = self.http("POST", "/v1/rooms", None, &[("x-lorekeeper-create-key", CREATE_KEY)], None).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let r: CreateRoomResponse = serde_json::from_value(body).unwrap();
        assert!(is_id(&r.room));
        assert!(decode_secret(&r.owner_token).is_ok());
        (r.room, r.owner_token)
    }

    /// Invites and redeems a member: their token.
    async fn member(&self, room: &str, owner: &str, name: &str) -> String {
        let (status, body) = self.http("POST", &format!("/v1/rooms/{room}/invites"), Some(owner), &[], None).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let invite = body["invite"].as_str().unwrap().to_string();
        let (status, body) = self.redeem(room, &invite, name).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        body["token"].as_str().unwrap().to_string()
    }

    async fn redeem(&self, room: &str, invite: &str, name: &str) -> (StatusCode, Value) {
        self.http("POST", &format!("/v1/rooms/{room}/invites/{invite}/redeem"), None, &[], Some(json!({ "member": name }))).await
    }

    async fn ws(&self, room: &str, token: &str) -> Ws {
        let mut req = format!("ws://{}/v1/rooms/{room}/live", self.addr).into_client_request().unwrap();
        req.headers_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());
        let (ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
        Ws { ws, room: room.into(), me: String::new(), log: Vec::new(), closed: None }
    }

    async fn ws_status(&self, room: &str, token: Option<&str>) -> StatusCode {
        let mut req = format!("ws://{}/v1/rooms/{room}/live", self.addr).into_client_request().unwrap();
        if let Some(t) = token {
            req.headers_mut().insert("authorization", format!("Bearer {t}").parse().unwrap());
        }
        match tokio_tungstenite::connect_async(req).await {
            Err(tungstenite::Error::Http(res)) => res.status(),
            Ok(_) => StatusCode::SWITCHING_PROTOCOLS,
            Err(e) => panic!("{e}"),
        }
    }
}

struct Ws {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    room: String,
    /// This socket's member id, from the access message after hello.
    me: String,
    /// Every text frame received, for checking what never arrived.
    log: Vec<String>,
    /// The close code, once the server closed the socket.
    closed: Option<u16>,
}

fn blob(room: &str, path: &str, text: &str) -> (String, String) {
    let id = file_id(&KEY, path);
    let file = FileContent { path: path.into(), content: text.as_bytes().to_vec(), modified: 1, ..Default::default() };
    let b = encode_blob(&seal(&KEY, room, &id, &file));
    (id, b)
}

fn private_blob(room: &str, member_id: &str, path: &str, text: &str) -> (String, String) {
    let id = private_file_id(&KEY, member_id, path);
    let file = FileContent { path: path.into(), content: text.as_bytes().to_vec(), modified: 1, ..Default::default() };
    let b = encode_blob(&seal(&KEY, room, &id, &file));
    (id, b)
}

fn text_of(room: &str, id: &str, blob: &str) -> String {
    let file = open(&KEY, room, id, &decode_blob(blob).unwrap()).unwrap();
    String::from_utf8(file.content).unwrap()
}

impl Ws {
    async fn send(&mut self, msg: ClientMessage) {
        self.ws.send(Message::text(serde_json::to_string(&msg).unwrap())).await.unwrap();
    }

    /// The next JSON message; `None` when the server closed the socket.
    async fn next(&mut self) -> Option<ServerMessage> {
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(5), self.ws.next()).await.expect("no message within 5 s");
            match msg {
                Some(Ok(Message::Text(t))) => {
                    self.log.push(t.to_string());
                    return Some(serde_json::from_str(t.as_str()).unwrap());
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                Some(Ok(Message::Close(frame))) => {
                    self.closed = frame.map(|f| u16::from(f.code));
                    return None;
                }
                None | Some(Err(_)) => return None,
                Some(Ok(other)) => panic!("unexpected {other:?}"),
            }
        }
    }

    async fn recv(&mut self) -> ServerMessage {
        self.next().await.expect("socket closed")
    }

    /// Says hello and checks the replay ends; returns (all replayed changes, presence).
    async fn hello(&mut self, since: u64, member: &str) -> (Vec<Change>, Vec<PresenceMember>) {
        self.hello_dm(since, None, member).await
    }

    /// Hello with `dm_since`: the access message comes first (its member id is kept in `me`), then the replay.
    async fn hello_dm(&mut self, since: u64, dm_since: Option<u64>, member: &str) -> (Vec<Change>, Vec<PresenceMember>) {
        self.send(ClientMessage::Hello { since, member: member.into(), dm_since }).await;
        match self.recv().await {
            ServerMessage::Access { member_id, .. } => self.me = member_id,
            other => panic!("expected access, got {other:?}"),
        }
        let mut all = Vec::new();
        loop {
            match self.recv().await {
                ServerMessage::Changes { changes, more, total, .. } => {
                    all.extend(changes);
                    if !more {
                        // The count the replay announced is what it sent: never a file this member may not see (the
                        // tests write nothing during a replay).
                        assert_eq!(total, Some(all.len() as u64), "replay total");
                        break;
                    }
                }
                other => panic!("expected changes, got {other:?}"),
            }
        }
        match self.recv().await {
            ServerMessage::Presence { members } => (all, members),
            other => panic!("expected presence, got {other:?}"),
        }
    }

    async fn put(&mut self, req: u64, path: &str, base: u64, text: &str) -> ServerMessage {
        let (id, blob) = blob(&self.room, path, text);
        self.send(ClientMessage::Put { req, id, base, blob, space: Space::Shared }).await;
        self.recv().await
    }

    async fn delete(&mut self, req: u64, path: &str, base: u64) -> ServerMessage {
        self.send(ClientMessage::Delete { req, id: file_id(&KEY, path), base, space: Space::Shared }).await;
        self.recv().await
    }

    /// The next message is a pong: nothing else was queued before it.
    async fn assert_quiet(&mut self) {
        self.send(ClientMessage::Ping).await;
        assert_eq!(self.recv().await, ServerMessage::Pong);
    }

    async fn presence(&mut self) -> Vec<String> {
        match self.recv().await {
            ServerMessage::Presence { members } => members.into_iter().map(|m| m.member).collect(),
            other => panic!("expected presence, got {other:?}"),
        }
    }
}

fn open_config(c: &mut Config) {
    c.create_key = Some(CREATE_KEY.into());
}

#[tokio::test]
async fn plain_endpoints() {
    let s = start(open_config).await;
    assert_eq!(s.http("GET", "/healthz", None, &[], None).await, (StatusCode::OK, Value::String("ok".into())));
    let app = router(s.state.clone());
    let res = app.oneshot(Request::get("/").body(Body::empty()).unwrap()).await.unwrap();
    assert!(res.status().is_redirection());
    assert_eq!(res.headers()["location"], "https://yonatankarp.com/lorekeeper/");

    // The join page explains itself and leaves the invite alone.
    let (room, owner) = s.room().await;
    let (_, inv) = s.http("POST", &format!("/v1/rooms/{room}/invites"), Some(&owner), &[], None).await;
    let invite = inv["invite"].as_str().unwrap();
    let (status, page) = s.http("GET", &format!("/join/{room}/{invite}"), None, &[], None).await;
    assert_eq!(status, StatusCode::OK);
    let page = page.as_str().unwrap();
    assert!(page.contains("Join a shared campaign") && page.contains("https://lorekeeper.yonatankarp.com"));
    assert!(!page.contains("http://") && !page.contains("src="), "no external assets");
    assert_eq!(s.redeem(&room, invite, "x").await.0, StatusCode::CREATED);
}

#[tokio::test]
async fn creation_key_gates_room_creation_only() {
    let s = start(open_config).await;
    let (status, body) = s.http("POST", "/v1/rooms", None, &[], None).await;
    assert_eq!((status, body), (StatusCode::FORBIDDEN, json!({ "error": "create_key_required" })));
    let wrong = &CREATE_KEY[1..];
    let (status, body) = s.http("POST", "/v1/rooms", None, &[("x-lorekeeper-create-key", wrong)], None).await;
    assert_eq!((status, body), (StatusCode::FORBIDDEN, json!({ "error": "create_key_required" })));
    let (room, owner) = s.room().await;
    // Joining and syncing never need it.
    let member = s.member(&room, &owner, "m").await;
    assert_eq!(s.http("GET", &format!("/v1/rooms/{room}/changes"), Some(&member), &[], None).await.0, StatusCode::OK);
    let mut ws = s.ws(&room, &member).await;
    ws.hello(0, "m").await;

    // Without a configured key, creation is closed, whatever the header says.
    let closed = start(|_| {}).await;
    assert_eq!(closed.http("POST", "/v1/rooms", None, &[], None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(closed.http("POST", "/v1/rooms", None, &[("x-lorekeeper-create-key", "")], None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(closed.http("POST", "/v1/rooms", None, &[("x-lorekeeper-create-key", CREATE_KEY)], None).await.0, StatusCode::FORBIDDEN);
}

#[test]
fn config_from_env_vars() {
    let vars = |pairs: &'static [(&'static str, &'static str)]| move |name: &str| pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string());
    // The server refuses to start without a creation key.
    assert!(Config::from_vars(vars(&[])).is_err());
    assert!(Config::from_vars(vars(&[("LOREKEEPER_SYNC_CREATE_KEY", "")])).is_err());
    assert!(Config::from_vars(vars(&[("LOREKEEPER_SYNC_CREATE_KEY", "short")])).is_err());
    let c = Config::from_vars(vars(&[("LOREKEEPER_SYNC_CREATE_KEY", CREATE_KEY)])).unwrap();
    assert_eq!(c.create_key.as_deref(), Some(CREATE_KEY), "used verbatim, / + = included");
    assert!(!c.trust_cloudflare, "CF-Connecting-IP is only trusted when asked for");
    assert_eq!(c.addr, SocketAddr::from(([0, 0, 0, 0], 8080)));
    assert_eq!((c.max_connections, c.max_connections_total), (32, 64));
    let c = Config::from_vars(vars(&[
        ("LOREKEEPER_SYNC_CREATE_KEY", CREATE_KEY),
        ("LOREKEEPER_SYNC_TRUST_PROXY", "cloudflare"),
        ("LOREKEEPER_SYNC_MAX_FILES", "7"),
        ("LOREKEEPER_SYNC_MAX_CONNECTIONS_TOTAL", "9"),
    ]))
    .unwrap();
    assert!(c.trust_cloudflare);
    assert_eq!((c.max_files, c.max_connections_total), (7, 9));
    assert!(Config::from_vars(vars(&[("LOREKEEPER_SYNC_CREATE_KEY", CREATE_KEY), ("LOREKEEPER_SYNC_TRUST_PROXY", "nginx")])).is_err());
    assert!(Config::from_vars(vars(&[("LOREKEEPER_SYNC_CREATE_KEY", CREATE_KEY), ("LOREKEEPER_SYNC_MAX_BLOB", "lots")])).is_err());
}

#[tokio::test]
async fn auth_failures_all_look_the_same() {
    let s = start(open_config).await;
    let (room, owner) = s.room().await;
    let unauthorized = (StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" }));
    let other_token = encode_secret(&random_secret());
    let changes = |room: &str| format!("/v1/rooms/{room}/changes?since=0");
    assert_eq!(s.http("GET", &changes(&room), None, &[], None).await, unauthorized);
    assert_eq!(s.http("GET", &changes(&room), Some(&other_token), &[], None).await, unauthorized);
    assert_eq!(s.http("GET", &changes(&room), Some("not a token"), &[], None).await, unauthorized);
    assert_eq!(s.http("GET", &changes(&random_id()), Some(&owner), &[], None).await, unauthorized, "unknown room");
    assert_eq!(s.http("GET", &changes("nope"), Some(&owner), &[], None).await, unauthorized, "malformed room");
    assert_eq!(s.http("GET", &changes(&room), Some(&owner), &[], None).await.0, StatusCode::OK);
    // Another room's owner token doesn't open this one.
    let (_, owner2) = s.room().await;
    assert_eq!(s.http("GET", &changes(&room), Some(&owner2), &[], None).await, unauthorized);

    assert_eq!(s.ws_status(&room, None).await, StatusCode::UNAUTHORIZED);
    assert_eq!(s.ws_status(&room, Some(&other_token)).await, StatusCode::UNAUTHORIZED);
    assert_eq!(s.ws_status(&random_id(), Some(&owner)).await, StatusCode::UNAUTHORIZED);
    assert_eq!(s.ws_status(&room, Some(&owner)).await, StatusCode::SWITCHING_PROTOCOLS);
}

#[tokio::test]
async fn live_token_from_subprotocol_never_the_url() {
    let s = start(open_config).await;
    let (room, owner) = s.room().await;
    let mut req = format!("ws://{}/v1/rooms/{room}/live", s.addr).into_client_request().unwrap();
    req.headers_mut().insert("sec-websocket-protocol", format!("lorekeeper, token.{owner}").parse().unwrap());
    let (ws, res) = tokio_tungstenite::connect_async(req).await.unwrap();
    assert_eq!(res.headers()["sec-websocket-protocol"], "lorekeeper", "the token is never echoed");
    let mut ws = Ws { ws, room: room.clone(), me: String::new(), log: Vec::new(), closed: None };
    ws.hello(0, "a").await;

    // Tokens in URLs end up in proxy logs, so the query string isn't read.
    let req = format!("ws://{}/v1/rooms/{room}/live?token={owner}", s.addr).into_client_request().unwrap();
    assert!(matches!(tokio_tungstenite::connect_async(req).await, Err(tungstenite::Error::Http(res)) if res.status() == StatusCode::UNAUTHORIZED));
}

#[tokio::test]
async fn owner_invites_members_and_removes_them() {
    let s = start(open_config).await;
    let (room, owner) = s.room().await;
    let invites = format!("/v1/rooms/{room}/invites");

    let (status, body) = s.http("POST", &invites, Some(&owner), &[], None).await;
    assert_eq!(status, StatusCode::CREATED);
    let inv: InviteInfo = serde_json::from_value(body).unwrap();
    assert!(is_id(&inv.invite));
    let week = 7 * 86_400_000;
    assert!((inv.expires - sync_server::now_ms() - week).abs() < 60_000);

    // A link built from it parses back.
    let link = Invite { server: "https://lorekeeper.yonatankarp.com".into(), room: room.clone(), invite: inv.invite.clone(), key: KEY }.link();
    assert_eq!(Invite::parse(&link).unwrap().invite, inv.invite);

    let (status, list) = s.http("GET", &invites, Some(&owner), &[], None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list, json!([{ "invite": inv.invite, "expires": inv.expires, "role": "player", "manage": false }]));

    // Redeem once.
    let (status, body) = s.redeem(&room, &inv.invite, "enc(Brenna)").await;
    assert_eq!(status, StatusCode::CREATED);
    let member = body["token"].as_str().unwrap().to_string();
    assert_eq!(s.redeem(&room, &inv.invite, "enc(Eve)").await, (StatusCode::GONE, json!({ "error": "invite_used" })));
    assert_eq!(s.http("GET", &invites, Some(&owner), &[], None).await.1, json!([]));
    let not_found = (StatusCode::NOT_FOUND, json!({ "error": "invite_not_found" }));
    assert_eq!(s.redeem(&room, &random_id(), "x").await, not_found);
    assert_eq!(s.redeem(&random_id(), &inv.invite, "x").await, not_found, "invite of another room");
    assert_eq!(s.redeem(&room, "junk", "x").await, not_found);

    // Players can sync but not administer.
    let owner_only = (StatusCode::FORBIDDEN, json!({ "error": "not_allowed" }));
    assert_eq!(s.http("POST", &invites, Some(&member), &[], None).await, owner_only);
    assert_eq!(s.http("GET", &invites, Some(&member), &[], None).await, owner_only);
    assert_eq!(s.http("GET", &format!("/v1/rooms/{room}/members"), Some(&member), &[], None).await, owner_only);
    assert_eq!(s.http("GET", &format!("/v1/rooms/{room}/changes"), Some(&member), &[], None).await.0, StatusCode::OK);

    // Revoking a pending invite.
    let (_, body) = s.http("POST", &invites, Some(&owner), &[], None).await;
    let pending = body["invite"].as_str().unwrap();
    assert_eq!(s.http("DELETE", &format!("{invites}/{pending}"), Some(&owner), &[], None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(s.http("DELETE", &format!("{invites}/{pending}"), Some(&owner), &[], None).await, not_found);
    assert_eq!(s.redeem(&room, pending, "x").await, not_found);

    // Members list.
    let (status, list) = s.http("GET", &format!("/v1/rooms/{room}/members"), Some(&owner), &[], None).await;
    assert_eq!(status, StatusCode::OK);
    let list: Vec<MemberInfo> = serde_json::from_value(list).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!((list[0].role, list[0].member.as_str(), list[0].last_seen), (Role::Owner, "", None));
    assert_eq!((list[1].role, list[1].member.as_str()), (Role::Player, "enc(Brenna)"));
    let (owner_id, member_id) = (list[0].member_id.clone(), list[1].member_id.clone());

    // Connecting bumps last_seen; hello updates the display id.
    let mut ws = s.ws(&room, &member).await;
    ws.hello(0, "enc(Brenna 2)").await;
    let (_, list) = s.http("GET", &format!("/v1/rooms/{room}/members"), Some(&owner), &[], None).await;
    assert!(list[1]["last_seen"].is_i64());
    assert_eq!(list[1]["member"], "enc(Brenna 2)");

    // Removing: the owner can't remove themselves; a removed member is out at once.
    let members = format!("/v1/rooms/{room}/members");
    assert_eq!(s.http("DELETE", &format!("{members}/{owner_id}"), Some(&owner), &[], None).await, (StatusCode::BAD_REQUEST, json!({ "error": "cannot_remove_owner" })));
    assert_eq!(s.http("DELETE", &format!("{members}/{member_id}"), Some(&member), &[], None).await, owner_only);
    assert_eq!(s.http("DELETE", &format!("{members}/{member_id}"), Some(&owner), &[], None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(ws.next().await, None, "the removed member's socket closes");
    assert_eq!(s.http("DELETE", &format!("{members}/{member_id}"), Some(&owner), &[], None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(s.http("GET", &format!("/v1/rooms/{room}/changes"), Some(&member), &[], None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(s.ws_status(&room, Some(&member)).await, StatusCode::UNAUTHORIZED);
    assert_eq!(s.http("GET", &members, Some(&owner), &[], None).await.1.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn expired_invites_fail() {
    let s = start(|c| {
        open_config(c);
        c.invite_days = 0;
    })
    .await;
    let (room, owner) = s.room().await;
    let (_, body) = s.http("POST", &format!("/v1/rooms/{room}/invites"), Some(&owner), &[], None).await;
    let invite = body["invite"].as_str().unwrap();
    assert_eq!(s.redeem(&room, invite, "x").await, (StatusCode::GONE, json!({ "error": "invite_expired" })));
    assert_eq!(s.http("GET", &format!("/v1/rooms/{room}/invites"), Some(&owner), &[], None).await.1, json!([]));
}

#[tokio::test]
async fn sync_over_the_socket() {
    let s = start(open_config).await;
    let (room, owner) = s.room().await;
    let member = s.member(&room, &owner, "enc(B)").await;

    let mut a = s.ws(&room, &owner).await;
    let (replay, presence) = a.hello(0, "enc(A)").await;
    assert!(replay.is_empty());
    assert_eq!(presence.len(), 1);
    assert_eq!((presence[0].member.as_str(), presence[0].role), ("enc(A)", Role::Owner));

    assert_eq!(a.put(1, "Session 1/A.md", 0, "a1").await, ServerMessage::Ack { req: 1, seq: 1 });
    assert_eq!(a.put(2, "Session 1/A.md", 1, "a2").await, ServerMessage::Ack { req: 2, seq: 2 });
    assert_eq!(a.put(3, "Session 1/Other.md", 0, "o").await, ServerMessage::Ack { req: 3, seq: 3 });

    // Hello replays the latest version of each file after `since`.
    let mut b = s.ws(&room, &member).await;
    let (replay, presence) = b.hello(0, "enc(B)").await;
    let id_a = file_id(&KEY, "Session 1/A.md");
    assert_eq!(replay.iter().map(|c| c.seq).collect::<Vec<_>>(), [2, 3]);
    assert_eq!(text_of(&room, &id_a, replay[0].blob.as_ref().unwrap()), "a2");
    assert_eq!(presence.iter().map(|m| m.member.as_str()).collect::<Vec<_>>().len(), 2);
    assert_eq!(a.presence().await.len(), 2, "presence on join");
    let (replay, _) = s.ws(&room, &member).await.hello(2, "enc(B)").await;
    assert_eq!(replay.iter().map(|c| c.seq).collect::<Vec<_>>(), [3], "since skips what was seen");
    // That third connection closed again: presence is back to two members, unchanged.

    // Live fan-out: everyone else gets the change, including the writer's other connection, but
    // not the writing socket.
    let mut a2 = s.ws(&room, &owner).await;
    a2.hello(3, "enc(A)").await;
    assert_eq!(a.put(4, "Session 1/A.md", 2, "a3").await, ServerMessage::Ack { req: 4, seq: 4 });
    for ws in [&mut b, &mut a2] {
        match ws.recv().await {
            ServerMessage::Change(c) => {
                assert_eq!((c.id.as_str(), c.seq), (id_a.as_str(), 4));
                assert_eq!(text_of(&room, &id_a, &c.blob.unwrap()), "a3");
            }
            other => panic!("expected change, got {other:?}"),
        }
    }
    a.assert_quiet().await;

    // Conflict: B edits from seq 2 but the file is at 4; it gets the current version.
    match b.put(1, "Session 1/A.md", 2, "b's edit").await {
        ServerMessage::Conflict { req: 1, seq: 4, blob: Some(blob) } => assert_eq!(text_of(&room, &id_a, &blob), "a3"),
        other => panic!("expected conflict, got {other:?}"),
    }
    // New file over an existing one with base 0 is a conflict too.
    assert!(matches!(b.put(2, "Session 1/A.md", 0, "x").await, ServerMessage::Conflict { seq: 4, .. }));
    a.assert_quiet().await;

    // Delete leaves a tombstone that others learn about.
    assert_eq!(b.delete(3, "Session 1/A.md", 4).await, ServerMessage::Ack { req: 3, seq: 5 });
    assert_eq!(a.recv().await, ServerMessage::Change(Change { id: id_a.clone(), seq: 5, blob: None, author: None }));
    assert_eq!(a2.recv().await, ServerMessage::Change(Change { id: id_a.clone(), seq: 5, blob: None, author: None }));
    assert_eq!(b.delete(4, "Session 1/A.md", 4).await, ServerMessage::Conflict { req: 4, seq: 5, blob: None }, "stale delete");
    assert_eq!(b.delete(5, "Session 1/A.md", 5).await, ServerMessage::Ack { req: 5, seq: 5 }, "deleting again changes nothing");
    assert_eq!(b.delete(6, "Never.md", 0).await, ServerMessage::Ack { req: 6, seq: 0 }, "deleting a missing file changes nothing");
    let (status, body) = s.http("GET", &format!("/v1/rooms/{room}/changes?since=3"), Some(&member), &[], None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "seq": 5, "changes": [{ "id": id_a, "seq": 5, "blob": null }], "more": false }));

    // A file can come back over its tombstone, from base 0 (never saw it) or the tombstone's seq.
    assert_eq!(b.put(7, "Session 1/A.md", 0, "back").await, ServerMessage::Ack { req: 7, seq: 6 });
    assert!(matches!(a.recv().await, ServerMessage::Change(Change { seq: 6, .. })));

    // Presence on leave.
    drop(b);
    assert_eq!(a.presence().await, ["enc(A)"]);
}

#[tokio::test]
async fn socket_protocol_errors() {
    let s = start(open_config).await;
    let (room, owner) = s.room().await;
    let mut a = s.ws(&room, &owner).await;
    assert_eq!(a.put(1, "x.md", 0, "x").await, ServerMessage::Error { req: Some(1), error: "hello_first".into() });
    a.ws.send(Message::text("{\"type\":\"shout\"}")).await.unwrap();
    assert_eq!(a.recv().await, ServerMessage::Error { req: None, error: "bad_message".into() });
    a.hello(0, "a").await;
    a.send(ClientMessage::Hello { since: 0, member: "a".into(), dm_since: None }).await;
    assert_eq!(a.recv().await, ServerMessage::Error { req: None, error: "hello_twice".into() });
    a.send(ClientMessage::Put { req: 2, id: "bad".into(), base: 0, blob: "AAAA".into(), space: Space::Shared }).await;
    assert_eq!(a.recv().await, ServerMessage::Error { req: Some(2), error: "bad_id".into() });
    a.send(ClientMessage::Put { req: 3, id: random_id(), base: 0, blob: "not base64!".into(), space: Space::Shared }).await;
    assert_eq!(a.recv().await, ServerMessage::Error { req: Some(3), error: "bad_blob".into() });
    a.send(ClientMessage::Put { req: 4, id: random_id(), base: 0, blob: encode_blob(&[0u8; 39]), space: Space::Shared }).await;
    assert_eq!(a.recv().await, ServerMessage::Error { req: Some(4), error: "bad_blob".into() }, "shorter than nonce and tag");
    a.assert_quiet().await;
}

#[tokio::test]
async fn limits() {
    let s = start(|c| {
        open_config(c);
        c.max_blob = 1000;
        c.max_files = 2;
        c.max_room_bytes = 1400;
        c.max_connections = 2;
    })
    .await;
    let (room, owner) = s.room().await;
    let mut a = s.ws(&room, &owner).await;
    a.hello(0, "a").await;
    let err = |req, e: &str| ServerMessage::Error { req: Some(req), error: e.into() };

    let big = "x".repeat(1000);
    assert_eq!(a.put(1, "big.md", 0, &big).await, err(1, "too_large"));
    let fits = "x".repeat(500); // 749 bytes sealed
    assert!(matches!(a.put(2, "a.md", 0, &fits).await, ServerMessage::Ack { seq: 1, .. }));
    assert_eq!(a.put(3, "b.md", 0, &fits).await, err(3, "room_full"));
    assert!(matches!(a.put(4, "b.md", 0, "small").await, ServerMessage::Ack { seq: 2, .. }));
    assert_eq!(a.put(5, "c.md", 0, "small").await, err(5, "too_many_files"));
    // Deleting frees a file slot and its bytes; tombstones don't count.
    assert!(matches!(a.delete(6, "a.md", 1).await, ServerMessage::Ack { seq: 3, .. }));
    assert!(matches!(a.put(7, "c.md", 0, &fits).await, ServerMessage::Ack { seq: 4, .. }));

    // Connections per room.
    let _b = s.ws(&room, &owner).await;
    assert_eq!(s.ws_status(&room, Some(&owner)).await, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn connections_are_capped_per_member() {
    let s = start(open_config).await;
    let (room, owner) = s.room().await;
    let member = s.member(&room, &owner, "m").await;
    let mut held = Vec::new();
    for _ in 0..8 {
        held.push(s.ws(&room, &member).await);
    }
    assert_eq!(s.ws_status(&room, Some(&member)).await, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(s.ws_status(&room, Some(&owner)).await, StatusCode::SWITCHING_PROTOCOLS, "others still get in");
}

#[tokio::test]
async fn frames_over_the_size_limit_close_the_socket() {
    let s = start(|c| {
        open_config(c);
        c.max_blob = 1000;
    })
    .await;
    let (room, owner) = s.room().await;
    let mut a = s.ws(&room, &owner).await;
    a.hello(0, "a").await;
    let huge = "A".repeat(200_000);
    a.send(ClientMessage::Put { req: 1, id: random_id(), base: 0, blob: huge, space: Space::Shared }).await;
    assert_eq!(a.next().await, None);
}

#[tokio::test]
async fn write_rate_limit_per_room() {
    let s = start(|c| {
        open_config(c);
        c.writes_per_minute = 3;
    })
    .await;
    let (room, owner) = s.room().await;
    let mut a = s.ws(&room, &owner).await;
    a.hello(0, "a").await;
    for i in 1..=3 {
        assert!(matches!(a.put(i, &format!("{i}.md"), 0, "x").await, ServerMessage::Ack { .. }));
    }
    assert_eq!(a.put(4, "4.md", 0, "x").await, ServerMessage::Error { req: Some(4), error: "rate_limited".into() });
    // Another room has its own budget.
    let (room2, owner2) = s.room().await;
    let mut b = s.ws(&room2, &owner2).await;
    b.hello(0, "b").await;
    assert!(matches!(b.put(1, "1.md", 0, "x").await, ServerMessage::Ack { .. }));
}

#[tokio::test]
async fn room_creation_and_redeem_rate_limits_per_ip() {
    let s = start(|c| {
        open_config(c);
        c.rooms_per_hour = 2;
        c.redeems_per_hour = 2;
        c.trust_cloudflare = true;
    })
    .await;
    let srv = &s;
    let create = |ip: &'static str| async move {
        srv.http("POST", "/v1/rooms", None, &[("cf-connecting-ip", ip), ("x-lorekeeper-create-key", CREATE_KEY)], None).await.0
    };
    assert_eq!(create("203.0.113.1").await, StatusCode::CREATED);
    assert_eq!(create("203.0.113.1").await, StatusCode::CREATED);
    assert_eq!(create("203.0.113.1").await, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(create("203.0.113.2").await, StatusCode::CREATED, "CF-Connecting-IP is the client");
    // An IPv6 client is its /64: hopping addresses inside it doesn't buy more.
    assert_eq!(create("2001:db8:1:2::1").await, StatusCode::CREATED);
    assert_eq!(create("2001:db8:1:2:ffff::9").await, StatusCode::CREATED);
    assert_eq!(create("2001:db8:1:2:abcd::1").await, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(create("2001:db8:1:3::1").await, StatusCode::CREATED);

    let redeem = |ip: &'static str| async move {
        srv.http("POST", &format!("/v1/rooms/{}/invites/{}/redeem", random_id(), random_id()), None, &[("cf-connecting-ip", ip)], Some(json!({ "member": "x" }))).await.0
    };
    assert_eq!(redeem("203.0.113.9").await, StatusCode::NOT_FOUND);
    assert_eq!(redeem("203.0.113.9").await, StatusCode::NOT_FOUND);
    assert_eq!(redeem("203.0.113.9").await, StatusCode::TOO_MANY_REQUESTS);

    // Without a trusted proxy the header is ignored: everything comes from the peer address.
    // A wrong key is refused before the limit, so keyless requests can't use up an owner's budget.
    let keyed = start(|c| {
        open_config(c);
        c.rooms_per_hour = 1;
    })
    .await;
    let keyed = &keyed;
    let create = |key: &'static str| async move { keyed.http("POST", "/v1/rooms", None, &[("x-lorekeeper-create-key", key)], None).await.0 };
    assert_eq!(create("wrong").await, StatusCode::FORBIDDEN);
    assert_eq!(create(CREATE_KEY).await, StatusCode::CREATED);
    assert_eq!(create(CREATE_KEY).await, StatusCode::TOO_MANY_REQUESTS);

    // That's the default.
    let direct = start(|c| {
        open_config(c);
        c.rooms_per_hour = 1;
    })
    .await;
    let direct = &direct;
    let create = |ip: &'static str| async move {
        direct.http("POST", "/v1/rooms", None, &[("cf-connecting-ip", ip), ("x-lorekeeper-create-key", CREATE_KEY)], None).await.0
    };
    assert_eq!(create("203.0.113.1").await, StatusCode::CREATED);
    assert_eq!(create("203.0.113.2").await, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn changes_come_in_pages() {
    let s = start(|c| {
        open_config(c);
        c.writes_per_minute = 10_000;
        c.max_blob = 8 * 1024 * 1024;
    })
    .await;
    let (room, owner) = s.room().await;
    let mut a = s.ws(&room, &owner).await;
    a.hello(0, "a").await;
    for i in 0..502u64 {
        assert!(matches!(a.put(i, &format!("{i}.md"), 0, "x").await, ServerMessage::Ack { .. }));
    }
    // 500 small changes per page.
    let page = |since: u64| {
        let s = &s;
        let (room, owner) = (room.clone(), owner.clone());
        async move {
            let (_, body) = s.http("GET", &format!("/v1/rooms/{room}/changes?since={since}"), Some(&owner), &[], None).await;
            serde_json::from_value::<ChangesResponse>(body).unwrap()
        }
    };
    let p = page(0).await;
    assert_eq!((p.seq, p.changes.len(), p.more), (502, 500, true));
    let p = page(500).await;
    assert_eq!((p.changes.len(), p.more), (2, false));
    // Pages also stop at 4 MiB of blobs (always at least one change), whatever the blob limit.
    for i in 0..3u64 {
        assert!(matches!(a.put(1000 + i, &format!("big{i}.md"), 0, &"y".repeat(2_400_000)).await, ServerMessage::Ack { .. }));
    }
    let p = page(502).await;
    assert_eq!((p.changes.len(), p.more), (1, true));
    // The socket replay pages the same way.
    let mut b = s.ws(&room, &owner).await;
    let (replay, _) = b.hello(0, "b").await;
    assert_eq!(replay.len(), 505);
    assert_eq!(s.http("GET", &format!("/v1/rooms/{room}/changes?since=x"), Some(&owner), &[], None).await.0, StatusCode::BAD_REQUEST);
    // A since past what SQLite can hold is just "nothing new", not a database error.
    let p = page(u64::MAX).await;
    assert_eq!((p.seq, p.changes.len(), p.more), (505, 0, false));
}

#[tokio::test]
async fn shutdown_closes_sockets() {
    let s = start(open_config).await;
    let (room, owner) = s.room().await;
    let mut a = s.ws(&room, &owner).await;
    a.hello(0, "a").await;
    s.state.shut_down();
    assert_eq!(a.next().await, None);
}

#[tokio::test]
async fn connections_are_capped_server_wide() {
    let s = start(|c| {
        open_config(c);
        c.max_connections_total = 2;
    })
    .await;
    let (room, owner) = s.room().await;
    let (room2, owner2) = s.room().await;
    let a = s.ws(&room, &owner).await;
    let _b = s.ws(&room2, &owner2).await;
    assert_eq!(s.ws_status(&room2, Some(&owner2)).await, StatusCode::TOO_MANY_REQUESTS, "another room, same server");
    drop(a);
    // The slot frees once the server sees the socket go.
    let mut status = StatusCode::TOO_MANY_REQUESTS;
    for _ in 0..50 {
        status = s.ws_status(&room2, Some(&owner2)).await;
        if status == StatusCode::SWITCHING_PROTOCOLS {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(status, StatusCode::SWITCHING_PROTOCOLS);
}

/// A member who stops reading while the server sends to them still gets cut off when removed:
/// removal interrupts the stuck send, closes the socket and frees its slot.
#[tokio::test]
async fn removal_interrupts_a_stalled_reader() {
    let s = start(|c| {
        open_config(c);
        c.max_connections = 2;
        c.writes_per_minute = 10_000;
        c.max_blob = 4 * 1024 * 1024;
    })
    .await;
    let (room, owner) = s.room().await;
    let member = s.member(&room, &owner, "m").await;
    let mut m = s.ws(&room, &member).await;
    m.hello(0, "m").await;
    let mut a = s.ws(&room, &owner).await;
    assert_eq!(a.hello(0, "a").await.1.len(), 2);
    // m never reads again: about 40 MiB of changes fill the socket buffers and the server's send
    // to m blocks.
    for i in 0..10u64 {
        assert!(matches!(a.put(i + 1, &format!("{i}.md"), 0, &"z".repeat(3_000_000)).await, ServerMessage::Ack { .. }));
    }
    let (_, list) = s.http("GET", &format!("/v1/rooms/{room}/members"), Some(&owner), &[], None).await;
    let member_id = list[1]["member_id"].as_str().unwrap().to_string();
    assert_eq!(s.http("DELETE", &format!("/v1/rooms/{room}/members/{member_id}"), Some(&owner), &[], None).await.0, StatusCode::NO_CONTENT);
    let mut status = StatusCode::TOO_MANY_REQUESTS;
    for _ in 0..50 {
        status = s.ws_status(&room, Some(&owner)).await;
        if status == StatusCode::SWITCHING_PROTOCOLS {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(status, StatusCode::SWITCHING_PROTOCOLS, "the removed member's slot is free");
    drop(m);
}

#[tokio::test]
async fn concurrent_redeems_of_one_invite_admit_one() {
    let s = start(open_config).await;
    let (room, owner) = s.room().await;
    let (_, body) = s.http("POST", &format!("/v1/rooms/{room}/invites"), Some(&owner), &[], None).await;
    let invite = body["invite"].as_str().unwrap().to_string();
    let tries = (0..8).map(|i| {
        let (s, room, invite) = (&s, room.clone(), invite.clone());
        async move { s.redeem(&room, &invite, &format!("p{i}")).await.0 }
    });
    let results = futures_util::future::join_all(tries).await;
    assert_eq!(results.iter().filter(|st| **st == StatusCode::CREATED).count(), 1, "{results:?}");
    assert!(results.iter().all(|st| *st == StatusCode::CREATED || *st == StatusCode::GONE), "{results:?}");
    let (_, list) = s.http("GET", &format!("/v1/rooms/{room}/members"), Some(&owner), &[], None).await;
    assert_eq!(list.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn reads_are_rate_limited_per_member() {
    let s = start(open_config).await;
    let (room, owner) = s.room().await;
    let member = s.member(&room, &owner, "m").await;
    let changes = format!("/v1/rooms/{room}/changes");
    let _a = s.ws(&room, &member).await;
    for _ in 0..59 {
        assert_eq!(s.http("GET", &changes, Some(&member), &[], None).await.0, StatusCode::OK);
    }
    let limited = (StatusCode::TOO_MANY_REQUESTS, json!({ "error": "rate_limited" }));
    assert_eq!(s.http("GET", &changes, Some(&member), &[], None).await, limited);
    assert_eq!(s.ws_status(&room, Some(&member)).await, StatusCode::TOO_MANY_REQUESTS, "upgrades share the budget");
    assert_eq!(s.http("GET", &changes, Some(&owner), &[], None).await.0, StatusCode::OK, "per member");
}

#[tokio::test]
async fn socket_message_flood_closes_the_socket() {
    let s = start(|c| {
        open_config(c);
        c.max_connections = 1;
    })
    .await;
    let (room, owner) = s.room().await;
    let mut a = s.ws(&room, &owner).await;
    for _ in 0..700 {
        if a.ws.send(Message::text(r#"{"type":"ping"}"#)).await.is_err() {
            break;
        }
    }
    // Pongs up to the limit, then the server hangs up. (The client may not see the close: the
    // server drops the socket with the rest of the flood unread.)
    let mut pongs = 0;
    while let Ok(Some(Ok(msg))) = tokio::time::timeout(Duration::from_secs(2), a.ws.next()).await {
        if msg.is_text() {
            pongs += 1;
        }
    }
    assert!(pongs <= 600, "{pongs}");
    assert_eq!(s.ws_status(&room, Some(&owner)).await, StatusCode::SWITCHING_PROTOCOLS, "the flooder's slot is free");
}

#[tokio::test]
async fn tombstones_are_capped_per_room() {
    let s = start(|c| {
        open_config(c);
        c.max_files = 2;
    })
    .await;
    let (room, owner) = s.room().await;
    let mut a = s.ws(&room, &owner).await;
    a.hello(0, "a").await;
    // Put and delete four new files: four tombstones, no live files.
    for i in 0..4u64 {
        assert!(matches!(a.put(i * 2, &format!("{i}.md"), 0, "x").await, ServerMessage::Ack { .. }));
        let seq = i * 2 + 1;
        assert!(matches!(a.delete(i * 2 + 1, &format!("{i}.md"), seq).await, ServerMessage::Ack { .. }));
    }
    assert_eq!(a.put(9, "new.md", 0, "x").await, ServerMessage::Error { req: Some(9), error: "too_many_files".into() });
    // A deleted file can still come back over its own tombstone.
    assert!(matches!(a.put(10, "0.md", 0, "back").await, ServerMessage::Ack { .. }));
}

#[tokio::test]
async fn join_page_is_locked_down() {
    let s = start(open_config).await;
    let app = router(s.state.clone());
    let uri = format!("/join/{}/{}", random_id(), random_id());
    let res = app.oneshot(Request::get(uri).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let csp = res.headers()["content-security-policy"].to_str().unwrap().to_string();
    assert!(csp.contains("default-src 'none'") && csp.contains("frame-ancestors 'none'"), "{csp}");
    assert_eq!(res.headers()["x-content-type-options"], "nosniff");
    assert_eq!(res.headers()["referrer-policy"], "no-referrer");
    assert!(!res.headers().contains_key("access-control-allow-origin"), "no CORS anywhere");
    let page = String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();

    // Exactly one script, allowed by its hash: no inline script runs but this one, and nothing loads.
    assert_eq!(page.matches("<script").count(), 1);
    let script = page.split("<script>").nth(1).unwrap().split("</script>").next().unwrap();
    let hash = encode_blob(&<sha2::Sha256 as sha2::Digest>::digest(script.as_bytes()));
    assert!(csp.contains(&format!("script-src 'sha256-{hash}';")), "{csp}");
    assert!(!csp.contains("unsafe-eval") && !csp.contains("script-src 'unsafe-inline'"), "{csp}");
    // It hands the link to the app and nowhere else: no request, no log, no storage, no other page.
    assert!(script.contains("\"lorekeeper://join?link=\" + encodeURIComponent(location.origin + path + key)"));
    for never in ["fetch", "XMLHttpRequest", "sendBeacon", "console", "Storage", "cookie", "open(", "http", "src", "innerHTML", "postMessage"] {
        assert!(!script.contains(never), "{never}");
    }
    // It opens the app's link, fills in the button for when the browser didn't ask, and navigates nowhere else.
    assert!(script.contains("open.href = url;") && script.contains("open.hidden = false;"));
    assert_eq!(script.matches("location.replace(url);").count(), 1);
    assert_eq!(script.matches("replace(").count(), 1, "the one navigation is to the app's link");
    for never in ["location.assign", "location.href", "location =", "location=", "assign(", "click(", "setTimeout", "dispatchEvent"] {
        assert!(!script.contains(never), "navigates elsewhere: {never}");
    }
    // The fallbacks: the button the script fills, the download page, and pasting by hand.
    assert!(page.contains("id=\"open\"") && page.contains("Open in Lorekeeper"));
    assert!(page.contains("Your browser may ask to open Lorekeeper. If nothing happens, click <b>Open in Lorekeeper</b>."));
    assert!(page.contains("Needs Lorekeeper 0.7 or later. Don't have it, or have an older version? <a href=\"https://yonatankarp.com/lorekeeper/\" rel=\"noreferrer\">Download Lorekeeper</a>, then open this link again."));
    assert!(page.contains("Keep it within your party."));
    assert!(!page.contains("http-equiv"), "no meta refresh");
    assert!(page.contains("Join a shared campaign") && page.contains("the part after <b>#</b>"));
}

// ---------- private notes, roles and re-invites ----------

/// Writes as many as a test needs.
fn busy(c: &mut Config) {
    open_config(c);
    c.writes_per_minute = 100_000;
}

impl Server {
    /// Invites (`by` may be the owner or a manager) and redeems: the new member's token.
    async fn join(&self, room: &str, by: &str, role: &str, manage: bool, name: &str) -> String {
        let (status, body) =
            self.http("POST", &format!("/v1/rooms/{room}/invites"), Some(by), &[], Some(json!({ "role": role, "manage": manage }))).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert_eq!((body["role"].as_str(), body["manage"].as_bool()), (Some(role), Some(manage)));
        let (status, body) = self.redeem(room, body["invite"].as_str().unwrap(), name).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        body["token"].as_str().unwrap().to_string()
    }

    /// The member id of the member with display id `name`.
    async fn id_of(&self, room: &str, owner: &str, name: &str) -> String {
        let (_, list) = self.http("GET", &format!("/v1/rooms/{room}/members"), Some(owner), &[], None).await;
        let list: Vec<MemberInfo> = serde_json::from_value(list).unwrap();
        list.into_iter().find(|m| m.member == name).unwrap().member_id
    }

    /// `GET /changes` as raw text, so nothing in it escapes the leak checks.
    async fn changes_text(&self, room: &str, token: &str, since: u64) -> String {
        let (status, body) = self.http("GET", &format!("/v1/rooms/{room}/changes?since={since}"), Some(token), &[], None).await;
        assert_eq!(status, StatusCode::OK);
        body.to_string()
    }

    async fn set_dm_reads(&self, room: &str, token: &str, on: bool) -> (StatusCode, Value) {
        self.http("PATCH", &format!("/v1/rooms/{room}/settings"), Some(token), &[], Some(json!({ "dm_reads_private": on }))).await
    }
}

impl Ws {
    /// Everything that arrives until the answer to a ping: what the server had queued for this socket.
    async fn drain(&mut self) -> Vec<ServerMessage> {
        self.send(ClientMessage::Ping).await;
        let mut out = Vec::new();
        loop {
            match self.recv().await {
                ServerMessage::Pong => return out,
                m => out.push(m),
            }
        }
    }

    /// A private note: (answer, id, blob as sent).
    async fn private(&mut self, req: u64, path: &str, base: u64, text: &str) -> (ServerMessage, String, String) {
        let (id, blob) = private_blob(&self.room, &self.me, path, text);
        self.send(ClientMessage::Put { req, id: id.clone(), base, blob: blob.clone(), space: Space::Private }).await;
        (self.recv().await, id, blob)
    }

    /// The next live change, whatever comes before it (access, presence).
    async fn next_change(&mut self) -> Change {
        loop {
            if let ServerMessage::Change(c) = self.recv().await {
                return c;
            }
        }
    }

    async fn put_raw(&mut self, req: u64, id: &str, base: u64, blob: &str, space: Space) -> ServerMessage {
        self.send(ClientMessage::Put { req, id: id.into(), base, blob: blob.into(), space }).await;
        self.recv().await
    }
}

fn changes_of(msgs: &[ServerMessage]) -> Vec<Change> {
    msgs.iter().filter_map(|m| if let ServerMessage::Change(c) = m { Some(c.clone()) } else { None }).collect()
}

/// None of `secrets` (blobs, ids) is anywhere in `texts`.
fn never(texts: &[String], secrets: &[&str]) {
    for t in texts {
        for s in secrets {
            assert!(!t.contains(s), "leaked {s:.20}... in {t:.200}");
        }
    }
}

/// A player's private note goes to their own devices only: never to another player, on any path (live, replay,
/// `GET /changes`, conflict replies), and to DMs (and the owner) only while the room allows it. Nobody can write into
/// another member's space.
#[tokio::test]
async fn private_notes_reach_only_their_author_and_dms_when_allowed() {
    let s = start(busy).await;
    let (room, owner) = s.room().await;
    let dm = s.join(&room, &owner, "dm", false, "enc(D)").await;
    let a = s.join(&room, &owner, "player", false, "enc(A)").await;
    let b = s.join(&room, &owner, "player", false, "enc(B)").await;
    let (mut so, mut sd, mut sa, mut sa2, mut sb) =
        (s.ws(&room, &owner).await, s.ws(&room, &dm).await, s.ws(&room, &a).await, s.ws(&room, &a).await, s.ws(&room, &b).await);
    for (ws, name) in [(&mut so, "enc(O)"), (&mut sd, "enc(D)"), (&mut sa, "enc(A)"), (&mut sa2, "enc(A)"), (&mut sb, "enc(B)")] {
        ws.hello(0, name).await;
    }
    for ws in [&mut so, &mut sd, &mut sa, &mut sa2, &mut sb] {
        ws.drain().await; // presence updates
    }
    let a_id = sa.me.clone();

    // A writes a private note; A's other device gets it, marked with its author.
    let path = "Sessions/Session 1/A.md";
    let (ack, pid, pblob) = sa.private(1, path, 0, "Halia lies").await;
    assert_eq!(ack, ServerMessage::Ack { req: 1, seq: 1 });
    assert_eq!(changes_of(&sa2.drain().await), [Change { id: pid.clone(), seq: 1, blob: Some(pblob.clone()), author: Some(a_id.clone()) }]);
    let (mut a_new, mut b_new) = (s.ws(&room, &a).await, s.ws(&room, &b).await);
    let (replay, _) = a_new.hello(0, "enc(A)").await;
    assert_eq!(replay.iter().map(|c| (c.id.as_str(), c.author.as_deref())).collect::<Vec<_>>(), [(pid.as_str(), Some(a_id.as_str()))]);

    // Nobody else hears of it while the room doesn't allow DMs to read: not live, not in a replay, not over HTTP.
    assert!(matches!(so.put(1, "Lore/Marker.md", 0, "marker").await, ServerMessage::Ack { seq: 2, .. }));
    for ws in [&mut sd, &mut sb] {
        assert_eq!(changes_of(&ws.drain().await).iter().map(|c| c.seq).collect::<Vec<_>>(), [2], "only the shared marker");
    }
    so.drain().await;
    let (replay, _) = b_new.hello(0, "enc(B)").await;
    assert_eq!(replay.iter().map(|c| c.seq).collect::<Vec<_>>(), [2]);
    let mut http = vec![s.changes_text(&room, &b, 0).await, s.changes_text(&room, &dm, 0).await, s.changes_text(&room, &owner, 0).await];

    // B aims at A's note: a conflict reply only ever holds B's own version (none), and writes land in B's own space.
    let err = |req: u64| ServerMessage::Conflict { req, seq: 0, blob: None };
    assert_eq!(sb.put_raw(2, &pid, 1, &pblob, Space::Private).await, err(2), "stale base in B's own (empty) space");
    assert_eq!(sb.put_raw(3, &pid, 1, &pblob, Space::Shared).await, err(3), "and in the shared space");
    sb.send(ClientMessage::Delete { req: 4, id: pid.clone(), base: 1, space: Space::Private }).await;
    assert_eq!(sb.recv().await, err(4));
    // The DM can't write into A's space either: the same id goes into the DM's own.
    assert_eq!(sd.put_raw(5, &pid, 1, &pblob, Space::Private).await, err(5));
    let (dm_ack, _, dm_blob) = sd.private(6, path, 0, "the DM's own note under A's path").await;
    assert!(matches!(dm_ack, ServerMessage::Ack { seq: 3, .. }));
    let (replay, _) = s.ws(&room, &a).await.hello(0, "enc(A)").await;
    assert_eq!(replay.iter().filter(|c| c.author.is_some()).map(|c| c.blob.clone().unwrap()).collect::<Vec<_>>(), std::slice::from_ref(&pblob), "A's note is A's");
    assert!(changes_of(&sa.drain().await).iter().all(|c| c.author.is_none()), "A never gets the DM's private note");

    // Only the owner turns DM reading on.
    let owner_only = (StatusCode::FORBIDDEN, json!({ "error": "owner_only" }));
    assert_eq!(s.set_dm_reads(&room, &dm, true).await, owner_only);
    assert_eq!(s.set_dm_reads(&room, &b, true).await, owner_only);
    assert_eq!(s.set_dm_reads(&room, &owner, true).await, (StatusCode::OK, json!({ "dm_reads_private": true })));

    // The DM is told, with the members, and gets every private note so far at once. The owner, who hasn't said
    // they're the DM, is told the setting and gets nothing.
    let msgs = sd.drain().await;
    let ServerMessage::Access { role: Role::Dm, dm_reads_private: true, members, .. } = &msgs[0] else { panic!("{msgs:?}") };
    assert_eq!(members.len(), 4);
    assert_eq!(changes_of(&msgs).iter().map(|c| (c.seq, c.author.clone())).collect::<Vec<_>>(), [(1, Some(a_id.clone()))]);
    let msgs = so.drain().await;
    assert!(matches!(&msgs[..], [ServerMessage::Access { role: Role::Owner, owner_is_dm: false, dm_reads_private: true, members, .. }] if members.is_empty()), "{msgs:?}");
    http.push(s.changes_text(&room, &owner, 0).await);
    // Players are told the setting (labels must be truthful) but get no notes and no member list.
    for ws in [&mut sb, &mut sa] {
        let msgs = ws.drain().await;
        assert!(matches!(&msgs[..], [ServerMessage::Access { role: Role::Player, dm_reads_private: true, members, .. }] if members.is_empty()), "{msgs:?}");
    }
    let (replay, _) = s.ws(&room, &dm).await.hello(0, "enc(D)").await;
    assert!(replay.iter().any(|c| c.id == pid && c.blob.as_deref() == Some(pblob.as_str())), "a DM's replay has them");
    // dm_since 0 brings others' private notes back even when `since` is past them.
    let mut d3 = s.ws(&room, &dm).await;
    let (replay, _) = d3.hello_dm(3, Some(0), "enc(D)").await;
    assert_eq!(replay.iter().map(|c| c.seq).collect::<Vec<_>>(), [1]);
    assert!(s.changes_text(&room, &dm, 0).await.contains(&pblob));
    // A's new private note reaches the DM live, never B.
    let (_, p2id, p2blob) = sa.private(7, "NPCs/Halia.md", 0, "I think she's the cult leader").await;
    assert_eq!(sd.next_change().await.id, p2id);
    sa.drain().await;
    assert!(matches!(sa.put(70, "Lore/Marker 3.md", 0, "marker").await, ServerMessage::Ack { .. }));
    assert_eq!(sb.next_change().await.author, None, "B's next change is the shared marker, nothing private before it");
    sd.drain().await;

    // Off again: the DM is told, and nothing private reaches them any more.
    assert_eq!(s.set_dm_reads(&room, &owner, false).await.0, StatusCode::OK);
    assert!(matches!(&sd.drain().await[..], [ServerMessage::Access { dm_reads_private: false, members, .. }] if members.is_empty()));
    let (_, p3id, p3blob) = sa.private(8, "NPCs/Halia.md", 4, "no, it's the innkeeper").await;
    so.drain().await;
    assert!(matches!(so.put(9, "Lore/Marker 2.md", 0, "marker").await, ServerMessage::Ack { .. }));
    for ws in [&mut sd, &mut sb] {
        let got = changes_of(&ws.drain().await);
        assert!(got.iter().all(|c| c.author.is_none()), "{got:?}");
    }
    let dm_text = s.changes_text(&room, &dm, 0).await;
    assert!(!dm_text.contains(&pblob) && !dm_text.contains(&p3blob), "not over HTTP either");
    http.push(s.changes_text(&room, &b, 0).await);

    // Through all of it, nothing of A's notes ever reached B: no blob, no id.
    let secrets = [pid.as_str(), pblob.as_str(), p2id.as_str(), p2blob.as_str(), p3id.as_str(), p3blob.as_str(), dm_blob.as_str()];
    for log in [&sb.log, &b_new.log, &so.log, &http] {
        never(log, &secrets);
    }
}

/// Being the owner reads no private notes, the setting on or off. An owner who says they're also the DM (on their own
/// row, nobody else's) reads like one; saying they aren't stops it on open sockets at once.
#[tokio::test]
async fn the_owner_reads_private_notes_only_as_the_dm() {
    let s = start(busy).await;
    let (room, owner) = s.room().await;
    let a = s.join(&room, &owner, "player", false, "enc(A)").await;
    let mgr = s.join(&room, &owner, "dm", true, "enc(M)").await;
    let (mut so, mut sa) = (s.ws(&room, &owner).await, s.ws(&room, &a).await);
    so.hello(0, "enc(O)").await;
    sa.hello(0, "enc(A)").await;
    let owner_id = so.me.clone();
    let (_, pid, pblob) = sa.private(1, "NPCs/Vex.md", 0, "Vex is my brother").await;
    for on in [false, true, false] {
        assert_eq!(s.set_dm_reads(&room, &owner, on).await.0, StatusCode::OK);
        let got = changes_of(&so.drain().await);
        assert!(got.is_empty(), "setting {on}: {got:?}");
        assert!(!s.changes_text(&room, &owner, 0).await.contains(&pblob));
    }
    let (replay, _) = s.ws(&room, &owner).await.hello(0, "enc(O)").await;
    assert!(replay.iter().all(|c| c.author.is_none()));
    never(&so.log, &[pid.as_str(), pblob.as_str()]);

    // Only the owner says it, only on their own row; it shows in the members list and in access.
    let me = format!("/v1/rooms/{room}/members/{owner_id}");
    let a_row = format!("/v1/rooms/{room}/members/{}", s.id_of(&room, &owner, "enc(A)").await);
    assert_eq!(s.http("PATCH", &me, Some(&mgr), &[], Some(json!({ "owner_is_dm": true }))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(s.http("PATCH", &a_row, Some(&owner), &[], Some(json!({ "owner_is_dm": true }))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(s.http("PATCH", &me, Some(&owner), &[], Some(json!({ "owner_is_dm": true }))).await.0, StatusCode::NO_CONTENT);
    let msgs = so.drain().await;
    assert!(matches!(&msgs[..], [ServerMessage::Access { owner_is_dm: true, dm_reads_private: false, .. }]), "{msgs:?}");
    let (_, list) = s.http("GET", &format!("/v1/rooms/{room}/members"), Some(&owner), &[], None).await;
    assert_eq!(list[0]["owner_is_dm"], true);
    assert_eq!(list[1]["owner_is_dm"], false);

    // With the setting on, the owner who's the DM gets every private note at once, and new ones live.
    assert_eq!(s.set_dm_reads(&room, &owner, true).await.0, StatusCode::OK);
    let msgs = so.drain().await;
    assert!(matches!(&msgs[0], ServerMessage::Access { owner_is_dm: true, dm_reads_private: true, members, .. } if members.len() == 3), "{msgs:?}");
    assert_eq!(changes_of(&msgs).iter().map(|c| c.id.clone()).collect::<Vec<_>>(), std::slice::from_ref(&pid));
    let (_, p2, _) = sa.private(2, "NPCs/Vex 2.md", 0, "and my rival").await;
    assert_eq!(so.next_change().await.id, p2, "live, as it happens");

    // Not the DM any more: told at once, and nothing private reaches the open socket after that.
    assert_eq!(s.http("PATCH", &me, Some(&owner), &[], Some(json!({ "owner_is_dm": false }))).await.0, StatusCode::NO_CONTENT);
    assert!(matches!(&so.drain().await[..], [ServerMessage::Access { owner_is_dm: false, members, .. }] if members.is_empty()));
    let (_, p3, p3blob) = sa.private(3, "NPCs/Vex 3.md", 0, "no, my cousin").await;
    // A shared marker after it: the owner's next change is the marker, so nothing came before it.
    sa.drain().await;
    assert!(matches!(sa.put(4, "Lore/Marker.md", 0, "marker").await, ServerMessage::Ack { .. }));
    assert_eq!(so.next_change().await.author, None);
    assert!(!s.changes_text(&room, &owner, 0).await.contains(&p3blob));
    never(&so.log[so.log.len().saturating_sub(4)..], &[p3.as_str(), p3blob.as_str()]);
}

/// The owner makes a player a DM while the room allows DM reading: their open socket gets every private note at once.
/// Made a player again, they get nothing more.
#[tokio::test]
async fn a_role_change_takes_effect_on_open_sockets() {
    let s = start(busy).await;
    let (room, owner) = s.room().await;
    let a = s.join(&room, &owner, "player", false, "enc(A)").await;
    let c = s.join(&room, &owner, "player", false, "enc(C)").await;
    assert_eq!(s.set_dm_reads(&room, &owner, true).await.0, StatusCode::OK);
    let (mut sa, mut sc) = (s.ws(&room, &a).await, s.ws(&room, &c).await);
    sa.hello(0, "enc(A)").await;
    sc.hello(0, "enc(C)").await;
    sc.drain().await;
    let (_, pid, _) = sa.private(1, "NPCs/Vex.md", 0, "Vex is my brother").await;
    assert!(changes_of(&sc.drain().await).is_empty());
    let c_id = s.id_of(&room, &owner, "enc(C)").await;
    let member = format!("/v1/rooms/{room}/members/{c_id}");
    // Only the owner changes roles.
    assert_eq!(s.http("PATCH", &member, Some(&a), &[], Some(json!({ "role": "dm" }))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(s.http("PATCH", &member, Some(&owner), &[], Some(json!({ "role": "dm" }))).await.0, StatusCode::NO_CONTENT);
    let msgs = sc.drain().await;
    assert!(matches!(&msgs[0], ServerMessage::Access { role: Role::Dm, dm_reads_private: true, .. }), "{msgs:?}");
    assert_eq!(changes_of(&msgs).iter().map(|c| c.id.clone()).collect::<Vec<_>>(), [pid]);
    assert_eq!(s.http("PATCH", &member, Some(&owner), &[], Some(json!({ "role": "player" }))).await.0, StatusCode::NO_CONTENT);
    let msgs = sc.drain().await;
    assert!(msgs.iter().any(|m| matches!(m, ServerMessage::Access { role: Role::Player, .. })) && changes_of(&msgs).is_empty(), "{msgs:?}");
    let (_, p2, _) = sa.private(2, "NPCs/Vex 2.md", 0, "still my brother").await;
    sa.drain().await;
    let got = changes_of(&sc.drain().await);
    assert!(got.iter().all(|c| c.id != p2), "{got:?}");
}

/// Who may do what: players nothing, DMs nothing more unless the owner lets them manage; a manager invites players and
/// DMs, lists and cancels invites, removes and re-invites members who don't manage; only the owner grants manage,
/// changes roles and the room's setting; nobody removes, re-invites or changes the owner.
#[tokio::test]
async fn who_may_manage_members() {
    let s = start(busy).await;
    let (room, owner) = s.room().await;
    let mgr = s.join(&room, &owner, "dm", true, "enc(M)").await;
    s.join(&room, &owner, "dm", true, "enc(M2)").await;
    let dm = s.join(&room, &owner, "dm", false, "enc(D)").await;
    let p = s.join(&room, &mgr, "player", false, "enc(P)").await;
    let ids = |name: &'static str| {
        let (s, room, owner) = (&s, room.clone(), owner.clone());
        async move { s.id_of(&room, &owner, name).await }
    };
    let (owner_id, mgr2_id, dm_id, p_id) = (ids("").await, ids("enc(M2)").await, ids("enc(D)").await, ids("enc(P)").await);
    let invites = format!("/v1/rooms/{room}/invites");
    let members = format!("/v1/rooms/{room}/members");
    let not_allowed = (StatusCode::FORBIDDEN, json!({ "error": "not_allowed" }));
    let owner_only = (StatusCode::FORBIDDEN, json!({ "error": "owner_only" }));
    let bad = (StatusCode::BAD_REQUEST, json!({ "error": "bad_request" }));
    let post = |path: String, who: String, body: Value| {
        let s = &s;
        async move { s.http("POST", &path, Some(&who), &[], Some(body)).await }
    };

    // Players and plain DMs administer nothing.
    for who in [&p, &dm] {
        assert_eq!(post(invites.clone(), who.clone(), json!({})).await, not_allowed);
        assert_eq!(s.http("GET", &invites, Some(who), &[], None).await, not_allowed);
        assert_eq!(s.http("GET", &members, Some(who), &[], None).await, not_allowed);
        assert_eq!(s.http("DELETE", &format!("{members}/{p_id}"), Some(who), &[], None).await, not_allowed);
        assert_eq!(post(format!("{members}/{p_id}/reinvite"), who.clone(), json!({})).await, not_allowed);
    }
    // A manager invites players and DMs, but can't grant manage; nobody invites an owner or a managing player.
    assert_eq!(post(invites.clone(), mgr.clone(), json!({ "role": "dm" })).await.0, StatusCode::CREATED);
    assert_eq!(post(invites.clone(), mgr.clone(), json!({ "role": "dm", "manage": true })).await, owner_only);
    assert_eq!(post(invites.clone(), owner.clone(), json!({ "role": "owner" })).await, bad);
    assert_eq!(post(invites.clone(), owner.clone(), json!({ "role": "player", "manage": true })).await, bad);
    assert_eq!(post(invites.clone(), owner.clone(), json!({ "role": "wizard" })).await, bad);
    // An empty body is a player (older apps send none).
    let (status, body) = s.http("POST", &invites, Some(&owner), &[], None).await;
    assert_eq!((status, body["role"].as_str()), (StatusCode::CREATED, Some("player")));
    // Lists and cancels invites, except one that grants manage or re-invites a manager: a manager doesn't even see
    // those, since they hold the key and could redeem any invite they see (a second managing identity, or another
    // manager taken over).
    let (_, granting) = post(invites.clone(), owner.clone(), json!({ "role": "dm", "manage": true })).await;
    let (_, mgr2_re) = post(format!("{members}/{mgr2_id}/reinvite"), owner.clone(), json!({})).await;
    let (status, list) = s.http("GET", &invites, Some(&mgr), &[], None).await;
    assert_eq!((status, list.as_array().unwrap().len()), (StatusCode::OK, 2));
    assert!(list.as_array().unwrap().iter().all(|i| i["manage"] == false && i.get("member_id").is_none()), "{list}");
    let (_, all) = s.http("GET", &invites, Some(&owner), &[], None).await;
    assert_eq!(all.as_array().unwrap().len(), 4, "the owner sees them all");
    let plain = list.as_array().unwrap()[0]["invite"].as_str().unwrap().to_string();
    assert_eq!(s.http("DELETE", &format!("{invites}/{plain}"), Some(&mgr), &[], None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(s.http("DELETE", &format!("{invites}/{}", granting["invite"].as_str().unwrap()), Some(&mgr), &[], None).await, owner_only);
    assert_eq!(s.http("DELETE", &format!("{invites}/{}", mgr2_re["invite"].as_str().unwrap()), Some(&mgr), &[], None).await, owner_only);
    assert_eq!(s.http("DELETE", &format!("{invites}/{}", mgr2_re["invite"].as_str().unwrap()), Some(&owner), &[], None).await.0, StatusCode::NO_CONTENT);
    // Roles, manage and the setting are the owner's.
    let patch = |who: &String, id: &str, body: Value| {
        let (s, path, who) = (&s, format!("{members}/{id}"), who.clone());
        async move { s.http("PATCH", &path, Some(&who), &[], Some(body)).await }
    };
    assert_eq!(patch(&mgr, &p_id, json!({ "role": "dm" })).await, owner_only);
    assert_eq!(s.set_dm_reads(&room, &mgr, true).await, owner_only);
    assert_eq!(patch(&owner, &owner_id, json!({ "role": "player" })).await, (StatusCode::BAD_REQUEST, json!({ "error": "cannot_change_owner" })));
    assert_eq!(patch(&owner, &p_id, json!({ "role": "owner" })).await, bad);
    assert_eq!(patch(&owner, &p_id, json!({ "manage": true })).await, bad, "a player never manages");
    assert_eq!(patch(&owner, &random_id(), json!({ "role": "dm" })).await.0, StatusCode::NOT_FOUND);
    assert_eq!(patch(&owner, &dm_id, json!({ "manage": true })).await.0, StatusCode::NO_CONTENT);
    assert_eq!(patch(&owner, &dm_id, json!({ "manage": false })).await.0, StatusCode::NO_CONTENT);
    let (_, list) = s.http("GET", &members, Some(&mgr), &[], None).await;
    let list: Vec<MemberInfo> = serde_json::from_value(list).unwrap();
    let role_of = |id: &str| list.iter().find(|m| m.member_id == id).map(|m| (m.role, m.manage)).unwrap();
    assert_eq!([role_of(&owner_id), role_of(&mgr2_id), role_of(&dm_id), role_of(&p_id)], [(Role::Owner, false), (Role::Dm, true), (Role::Dm, false), (Role::Player, false)]);
    // Re-invites and removals: a manager handles players and plain DMs, never the owner or another manager.
    assert_eq!(post(format!("{members}/{mgr2_id}/reinvite"), mgr.clone(), json!({})).await, owner_only);
    assert_eq!(post(format!("{members}/{owner_id}/reinvite"), owner.clone(), json!({})).await, (StatusCode::BAD_REQUEST, json!({ "error": "cannot_reinvite_owner" })));
    assert_eq!(post(format!("{members}/{}/reinvite", random_id()), owner.clone(), json!({})).await.0, StatusCode::NOT_FOUND);
    let (status, re) = post(format!("{members}/{p_id}/reinvite"), mgr.clone(), json!({})).await;
    assert_eq!((status, re["member_id"].as_str(), re["role"].as_str()), (StatusCode::CREATED, Some(p_id.as_str()), Some("player")));
    assert_eq!(s.http("DELETE", &format!("{members}/{mgr2_id}"), Some(&mgr), &[], None).await, owner_only);
    assert_eq!(s.http("DELETE", &format!("{members}/{owner_id}"), Some(&mgr), &[], None).await, (StatusCode::BAD_REQUEST, json!({ "error": "cannot_remove_owner" })));
    assert_eq!(s.http("DELETE", &format!("{members}/{p_id}"), Some(&mgr), &[], None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(s.http("DELETE", &format!("{members}/{dm_id}"), Some(&mgr), &[], None).await.0, StatusCode::NO_CONTENT);
    // The removed player's re-invite went with them.
    assert_eq!(s.redeem(&room, re["invite"].as_str().unwrap(), "").await.0, StatusCode::NOT_FOUND);
    assert_eq!(s.http("DELETE", &format!("{members}/{mgr2_id}"), Some(&owner), &[], None).await.0, StatusCode::NO_CONTENT);
}

/// Re-invite: the new computer becomes the same member (id, role, private notes) and the old one is signed out at once
/// with its own close code; the invite works once.
#[tokio::test]
async fn a_reinvite_moves_a_member_to_a_new_computer() {
    let s = start(busy).await;
    let (room, owner) = s.room().await;
    let old = s.join(&room, &owner, "dm", false, "enc(A)").await;
    let mut sa = s.ws(&room, &old).await;
    sa.hello(0, "enc(A)").await;
    let (_, pid, pblob) = sa.private(1, "NPCs/Vex.md", 0, "mine").await;
    let a_id = sa.me.clone();
    let (status, re) = s.http("POST", &format!("/v1/rooms/{room}/members/{a_id}/reinvite"), Some(&owner), &[], None).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!((re["member_id"].as_str(), re["role"].as_str()), (Some(a_id.as_str()), Some("dm")));
    // It's listed with the member it hands over, and takes no seat.
    let (_, list) = s.http("GET", &format!("/v1/rooms/{room}/invites"), Some(&owner), &[], None).await;
    assert_eq!(list[0]["member_id"], a_id.as_str());
    let invite = re["invite"].as_str().unwrap();
    let (status, body) = s.redeem(&room, invite, "").await;
    assert_eq!(status, StatusCode::CREATED);
    let new = body["token"].as_str().unwrap().to_string();
    assert_eq!(s.redeem(&room, invite, "").await.0, StatusCode::GONE);
    // The old computer: closed with 4002, its token refused from now on.
    while sa.next().await.is_some() {}
    assert_eq!(sa.closed, Some(4002));
    assert_eq!(s.http("GET", &format!("/v1/rooms/{room}/changes"), Some(&old), &[], None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(s.ws_status(&room, Some(&old)).await, StatusCode::UNAUTHORIZED);
    // The new one is that member, with their role, display id and private notes.
    let mut sn = s.ws(&room, &new).await;
    let (replay, _) = sn.hello(0, "enc(A)").await;
    assert_eq!(sn.me, a_id);
    assert_eq!(replay, [Change { id: pid.clone(), seq: 1, blob: Some(pblob), author: Some(a_id.clone()) }]);
    assert!(matches!(sn.private(2, "NPCs/Vex.md", 1, "still mine").await.0, ServerMessage::Ack { seq: 2, .. }));
    let (_, list) = s.http("GET", &format!("/v1/rooms/{room}/members"), Some(&owner), &[], None).await;
    let list: Vec<MemberInfo> = serde_json::from_value(list).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!((list[1].member_id.as_str(), list[1].role, list[1].member.as_str()), (a_id.as_str(), Role::Dm, "enc(A)"));
}

/// A re-invite hands over the member as they were when it was made: one a manager made for a player who has since
/// become a manager (or a DM) no longer works, so a manager can't take over a manager by re-inviting them early.
#[tokio::test]
async fn a_reinvite_from_before_a_role_change_is_void() {
    let s = start(busy).await;
    let (room, owner) = s.room().await;
    let mgr = s.join(&room, &owner, "dm", true, "enc(M)").await;
    s.join(&room, &owner, "player", false, "enc(P)").await;
    let p_id = s.id_of(&room, &owner, "enc(P)").await;
    let member = format!("/v1/rooms/{room}/members/{p_id}");
    let (status, re) = s.http("POST", &format!("{member}/reinvite"), Some(&mgr), &[], None).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(s.http("PATCH", &member, Some(&owner), &[], Some(json!({ "role": "dm", "manage": true }))).await.0, StatusCode::NO_CONTENT);
    let (status, body) = s.redeem(&room, re["invite"].as_str().unwrap(), "enc(M)").await;
    assert_eq!((status, body), (StatusCode::NOT_FOUND, json!({ "error": "invite_not_found" })));
    let (_, list) = s.http("GET", &format!("/v1/rooms/{room}/members"), Some(&owner), &[], None).await;
    let list: Vec<MemberInfo> = serde_json::from_value(list).unwrap();
    assert!(list.iter().any(|m| m.member_id == p_id && m.member == "enc(P)" && m.manage), "P is still P, a manager");
    // Made a DM (no manage) after a player's re-invite: also void; a fresh one works.
    s.join(&room, &owner, "player", false, "enc(Q)").await;
    let q_id = s.id_of(&room, &owner, "enc(Q)").await;
    let q = format!("/v1/rooms/{room}/members/{q_id}");
    let (_, re) = s.http("POST", &format!("{q}/reinvite"), Some(&owner), &[], None).await;
    assert_eq!(s.http("PATCH", &q, Some(&owner), &[], Some(json!({ "role": "dm" }))).await.0, StatusCode::NO_CONTENT);
    assert_eq!(s.redeem(&room, re["invite"].as_str().unwrap(), "").await.0, StatusCode::NOT_FOUND);
    let (_, re) = s.http("POST", &format!("{q}/reinvite"), Some(&owner), &[], None).await;
    assert_eq!(s.redeem(&room, re["invite"].as_str().unwrap(), "").await.0, StatusCode::CREATED);
}

/// Removing a member deletes their private space from the server (and frees its room size), and DMs who read private
/// notes are told the member list changed so they can drop their copies.
#[tokio::test]
async fn removal_deletes_the_private_space() {
    let s = start(|c| {
        busy(c);
        c.max_blob = 3000;
        c.max_room_bytes = 4000;
    })
    .await;
    let (room, owner) = s.room().await;
    let dm = s.join(&room, &owner, "dm", false, "enc(D)").await;
    let a = s.join(&room, &owner, "player", false, "enc(A)").await;
    assert_eq!(s.set_dm_reads(&room, &owner, true).await.0, StatusCode::OK);
    let (mut sa, mut sd, mut so) = (s.ws(&room, &a).await, s.ws(&room, &dm).await, s.ws(&room, &owner).await);
    sa.hello(0, "enc(A)").await;
    sd.hello(0, "enc(D)").await;
    so.hello(0, "enc(O)").await;
    let (_, _, pblob) = sa.private(1, "Lore/Notes.md", 0, &"x".repeat(1600)).await;
    assert_eq!(so.put(2, "Lore/Big.md", 0, &"y".repeat(1500)).await, ServerMessage::Error { req: Some(2), error: "room_full".into() });
    sd.drain().await;
    let a_id = sa.me.clone();
    assert_eq!(s.http("DELETE", &format!("/v1/rooms/{room}/members/{a_id}"), Some(&owner), &[], None).await.0, StatusCode::NO_CONTENT);
    while sa.next().await.is_some() {}
    assert_eq!(sa.closed, Some(4001));
    // The DM learns the members without A.
    let msgs = sd.drain().await;
    let Some(ServerMessage::Access { members, .. }) = msgs.iter().find(|m| matches!(m, ServerMessage::Access { .. })) else { panic!("{msgs:?}") };
    assert!(members.iter().all(|m| m.member_id != a_id) && members.len() == 2);
    // Gone from every read, and its bytes are free again.
    for token in [&owner, &dm] {
        assert!(!s.changes_text(&room, token, 0).await.contains(&pblob));
    }
    let (replay, _) = s.ws(&room, &dm).await.hello(0, "enc(D)").await;
    assert!(replay.is_empty());
    so.drain().await;
    assert!(matches!(so.put(3, "Lore/Big.md", 0, &"y".repeat(1500)).await, ServerMessage::Ack { .. }));
}

// ---------- deleting a room and leaving it ----------

/// Only the owner deletes a room. Everything in it goes at once: files (shared and private), members, tokens and
/// invites; every socket closes with 4003; afterwards the room answers like one that never existed, and the
/// server-wide connection budget it held is free again.
#[tokio::test]
async fn the_owner_deletes_the_room_and_everything_in_it() {
    let s = start(|c| {
        busy(c);
        c.max_connections_total = 4;
    })
    .await;
    let (room, owner) = s.room().await;
    let manager = s.join(&room, &owner, "dm", true, "enc(M)").await;
    let player = s.join(&room, &owner, "player", false, "enc(P)").await;
    let (_, pending) = s.http("POST", &format!("/v1/rooms/{room}/invites"), Some(&owner), &[], None).await;
    let pending = pending["invite"].as_str().unwrap().to_string();
    let (mut so, mut sm, mut sp) = (s.ws(&room, &owner).await, s.ws(&room, &manager).await, s.ws(&room, &player).await);
    so.hello(0, "enc(O)").await;
    sm.hello(0, "enc(M)").await;
    sp.hello(0, "enc(P)").await;
    so.drain().await; // presence
    assert!(matches!(so.put(1, "Lore/World.md", 0, "shared").await, ServerMessage::Ack { .. }));
    sp.drain().await;
    assert!(matches!(sp.private(2, "Secrets.md", 0, "mine").await.0, ServerMessage::Ack { .. }));

    // Managers and players can't; nothing changes.
    let room_url = format!("/v1/rooms/{room}");
    let owner_only = (StatusCode::FORBIDDEN, json!({ "error": "owner_only" }));
    assert_eq!(s.http("DELETE", &room_url, Some(&manager), &[], None).await, owner_only);
    assert_eq!(s.http("DELETE", &room_url, Some(&player), &[], None).await, owner_only);
    assert_eq!(s.http("DELETE", &room_url, None, &[], None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(s.http("DELETE", &format!("/v1/rooms/{}", random_id()), Some(&owner), &[], None).await.0, StatusCode::UNAUTHORIZED);
    assert!(s.changes_text(&room, &player, 0).await.contains("\"seq\":2"));

    // The owner can: every socket closes with 4003.
    assert_eq!(s.http("DELETE", &room_url, Some(&owner), &[], None).await.0, StatusCode::NO_CONTENT);
    for ws in [&mut so, &mut sm, &mut sp] {
        while ws.next().await.is_some() {}
        assert_eq!(ws.closed, Some(4003));
    }
    // Gone like a room that never existed: every token is refused, and the invite with it.
    let unauthorized = (StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" }));
    for token in [&owner, &manager, &player] {
        assert_eq!(s.http("GET", &format!("{room_url}/changes"), Some(token), &[], None).await, unauthorized);
        assert_eq!(s.ws_status(&room, Some(token)).await, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(s.http("DELETE", &room_url, Some(&owner), &[], None).await, unauthorized);
    assert_eq!(s.redeem(&room, &pending, "x").await.0, StatusCode::NOT_FOUND);
    // The connections it held are free: another room fills the server's budget of 4 again.
    let (room2, owner2) = s.room().await;
    let mut socks = Vec::new();
    for _ in 0..4 {
        let mut ws = s.ws(&room2, &owner2).await;
        ws.hello(0, "enc(O2)").await;
        socks.push(ws);
    }
    assert_eq!(s.ws_status(&room2, Some(&owner2)).await, StatusCode::TOO_MANY_REQUESTS);
}

/// A member leaves on their own: their token stops working, their sockets close with 4001, their private notes go,
/// and DMs who read them are told. Nobody else is touched; the owner can't leave (they delete the room).
#[tokio::test]
async fn a_member_leaves_on_their_own() {
    let s = start(busy).await;
    let (room, owner) = s.room().await;
    let dm = s.join(&room, &owner, "dm", false, "enc(D)").await;
    let a = s.join(&room, &owner, "player", false, "enc(A)").await;
    let b = s.join(&room, &owner, "player", false, "enc(B)").await;
    assert_eq!(s.set_dm_reads(&room, &owner, true).await.0, StatusCode::OK);
    let (mut sa, mut sd) = (s.ws(&room, &a).await, s.ws(&room, &dm).await);
    sa.hello(0, "enc(A)").await;
    sd.hello(0, "enc(D)").await;
    let (_, _, pblob) = sa.private(1, "Secrets.md", 0, "mine").await;
    sd.drain().await;
    let me = format!("/v1/rooms/{room}/members/me");

    assert_eq!(s.http("DELETE", &me, Some(&owner), &[], None).await, (StatusCode::BAD_REQUEST, json!({ "error": "owner_cannot_leave" })));
    assert_eq!(s.http("DELETE", &me, None, &[], None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(s.http("DELETE", &me, Some(&a), &[], None).await.0, StatusCode::NO_CONTENT);
    while sa.next().await.is_some() {}
    assert_eq!(sa.closed, Some(4001));
    assert_eq!(s.http("DELETE", &me, Some(&a), &[], None).await.0, StatusCode::UNAUTHORIZED, "the token is gone");
    let msgs = sd.drain().await;
    assert!(msgs.iter().any(|m| matches!(m, ServerMessage::Access { members, .. } if members.len() == 3)), "{msgs:?}");
    assert!(!s.changes_text(&room, &dm, 0).await.contains(&pblob), "their private notes went");
    // Everyone else stays.
    let (_, list) = s.http("GET", &format!("/v1/rooms/{room}/members"), Some(&owner), &[], None).await;
    assert_eq!(list.as_array().unwrap().len(), 3);
    assert_eq!(s.http("GET", &format!("/v1/rooms/{room}/changes"), Some(&b), &[], None).await.0, StatusCode::OK);
}

/// Deleting rooms and leaving them share a per-IP budget of 20 an hour.
#[tokio::test]
async fn deletes_are_rate_limited_per_ip() {
    let s = start(|c| {
        open_config(c);
        c.rooms_per_hour = 100;
    })
    .await;
    for _ in 0..20 {
        let (room, owner) = s.room().await;
        assert_eq!(s.http("DELETE", &format!("/v1/rooms/{room}"), Some(&owner), &[], None).await.0, StatusCode::NO_CONTENT);
    }
    let (room, owner) = s.room().await;
    let player = s.member(&room, &owner, "p").await;
    assert_eq!(s.http("DELETE", &format!("/v1/rooms/{room}"), Some(&owner), &[], None).await.0, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(s.http("DELETE", &format!("/v1/rooms/{room}/members/me"), Some(&player), &[], None).await.0, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(s.http("GET", &format!("/v1/rooms/{room}/changes"), Some(&owner), &[], None).await.0, StatusCode::OK, "the room stays");
}
