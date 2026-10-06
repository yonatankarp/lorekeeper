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
        Ws { ws, room: room.into() }
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
}

fn blob(room: &str, path: &str, text: &str) -> (String, String) {
    let id = file_id(&KEY, path);
    let file = FileContent { path: path.into(), content: text.as_bytes().to_vec(), modified: 1 };
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
                Some(Ok(Message::Text(t))) => return Some(serde_json::from_str(t.as_str()).unwrap()),
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return None,
                Some(Ok(other)) => panic!("unexpected {other:?}"),
            }
        }
    }

    async fn recv(&mut self) -> ServerMessage {
        self.next().await.expect("socket closed")
    }

    /// Says hello and checks the replay ends; returns (all replayed changes, presence).
    async fn hello(&mut self, since: u64, member: &str) -> (Vec<Change>, Vec<PresenceMember>) {
        self.send(ClientMessage::Hello { since, member: member.into() }).await;
        let mut all = Vec::new();
        loop {
            match self.recv().await {
                ServerMessage::Changes { changes, more, .. } => {
                    all.extend(changes);
                    if !more {
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
        self.send(ClientMessage::Put { req, id, base, blob }).await;
        self.recv().await
    }

    async fn delete(&mut self, req: u64, path: &str, base: u64) -> ServerMessage {
        self.send(ClientMessage::Delete { req, id: file_id(&KEY, path), base }).await;
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
    let mut ws = Ws { ws, room: room.clone() };
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
    assert_eq!(list, json!([{ "invite": inv.invite, "expires": inv.expires }]));

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

    // Members can sync but not administer.
    let owner_only = (StatusCode::FORBIDDEN, json!({ "error": "owner_only" }));
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
    assert_eq!((list[1].role, list[1].member.as_str()), (Role::Member, "enc(Brenna)"));
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
    assert_eq!(a.recv().await, ServerMessage::Change(Change { id: id_a.clone(), seq: 5, blob: None }));
    assert_eq!(a2.recv().await, ServerMessage::Change(Change { id: id_a.clone(), seq: 5, blob: None }));
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
    a.send(ClientMessage::Hello { since: 0, member: "a".into() }).await;
    assert_eq!(a.recv().await, ServerMessage::Error { req: None, error: "hello_twice".into() });
    a.send(ClientMessage::Put { req: 2, id: "bad".into(), base: 0, blob: "AAAA".into() }).await;
    assert_eq!(a.recv().await, ServerMessage::Error { req: Some(2), error: "bad_id".into() });
    a.send(ClientMessage::Put { req: 3, id: random_id(), base: 0, blob: "not base64!".into() }).await;
    assert_eq!(a.recv().await, ServerMessage::Error { req: Some(3), error: "bad_blob".into() });
    a.send(ClientMessage::Put { req: 4, id: random_id(), base: 0, blob: encode_blob(&[0u8; 39]) }).await;
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
    a.send(ClientMessage::Put { req: 1, id: random_id(), base: 0, blob: huge }).await;
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
    let csp = res.headers()["content-security-policy"].to_str().unwrap();
    assert!(csp.contains("default-src 'none'") && csp.contains("frame-ancestors 'none'"), "{csp}");
    assert_eq!(res.headers()["x-content-type-options"], "nosniff");
    assert!(!res.headers().contains_key("access-control-allow-origin"), "no CORS anywhere");
}
