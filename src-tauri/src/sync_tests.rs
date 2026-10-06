//! The sync engine against the real server, in-process on a temporary database: two players share, invite, join,
//! write, conflict, go offline, delete and get removed; a rogue member sends what the protocol allows and more.

use std::net::SocketAddr;
use std::sync::Mutex;

use futures_util::{SinkExt, StreamExt};
use sync_protocol::{random_secret, Invite};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use super::*;

// ---------- what syncs ----------

#[test]
fn only_safe_pages_and_images_sync() {
    for ok in [
        "NPCs/Vex.md", "Sessions/Session 4/Sibling 5.md", "Attachments/map.PNG", "PCs/Arn (12345678).md", METADATA, "Readme.md",
        "Quests/Border Conflict.md", "Lore/Ünïcode.md",
    ] {
        assert!(syncs(ok), "{ok}");
    }
    let long_name = format!("NPCs/{}.md", "a".repeat(253));
    let long_path = format!("{}/x.md", vec!["abc"; 300].join("/"));
    for bad in [
        "", "../escape.md", "NPCs/../../escape.md", "/etc/passwd.md", "C:/x.md", "C:\\x.md", "\\\\server\\share\\x.md", "NPCs//Vex.md",
        "./NPCs/Vex.md", "NPCs/./Vex.md", "NPCs/Vex.md/", ".obsidian/app.md", "NPCs/.hidden.md", ".lorekeeper/other.json",
        ".lorekeeper/campaign.json/x.md", "Templates/NPC.md", "NPCs/evil.sh", "NPCs/evil.exe", "NPCs/page.MD.exe", "notes.txt",
        "NPCs/Vex", "CON.md", "NPCs/lpt1.png", "NPCs/a:b.md", "NPCs/a*b.md", "NPCs/tab\there.md", "NPCs/nul\0.md", "NPCs/dot. /x.md",
        "NPCs/trailing /x.md", "NPCs/Vex (conflict 2026-10-06 2015).md", "NPCs/Vex (Mirela's conflicted copy 2026-10-06).md",
        "NPCs/Vex.sync-conflict-20261006-201500-ABCDEFG.md", "Lore/Gods [Conflict].md", &long_name, &long_path,
    ] {
        assert!(!syncs(bad), "{bad:?}");
    }
}

/// The same names as "sync conflict copies are found by their names" in vault.test.js; the Rust filter matches
/// the named patterns there ("Vex (1).md" is only a copy next to its original, which a filter can't know).
#[test]
fn conflict_copies_match_vault_js() {
    let copies = [
        "Vex (Mirela's conflicted copy 2026-10-06).md", "Vex.sync-conflict-20261006-201500-ABCDEFG.md", "Gods [Conflict].md",
        "Old (conflict 2026-10-06).md", "Vex (conflict 2026-10-06 2015).md", "Vex (conflict 2026-10-06 2015 2).md",
    ];
    let not = ["Vex.md", "Vex (1).md", "Border Conflict.md", "Border Conflict (2).md", "Arn (12345678).md", "Bob (1).md", "(Nonconflict).md"];
    for name in copies {
        assert!(is_conflict_copy(name), "{name}");
    }
    for name in not {
        assert!(!is_conflict_copy(name), "{name}");
    }
    assert_eq!(conflict_name("NPCs/Vex.md", "2026-10-06 2015", 1), "NPCs/Vex (conflict 2026-10-06 2015).md");
    assert_eq!(conflict_name("NPCs/Vex.md", "2026-10-06 2015", 2), "NPCs/Vex (conflict 2026-10-06 2015 2).md");
    assert_eq!(conflict_name("map.png", "2026-10-06 2015", 1), "map (conflict 2026-10-06 2015).png");
    assert!(is_conflict_copy(&conflict_name("Vex.md", "2026-10-06 2015", 3)));
}

#[test]
fn campaign_names_from_the_network_are_safe_folder_names() {
    assert_eq!(folder_name("  Curse of Strahd "), Some("Curse of Strahd".into()));
    assert_eq!(folder_name("Waterdeep."), Some("Waterdeep".into()));
    assert_eq!(folder_name(&"x".repeat(200)).map(|n| n.len()), Some(80));
    for bad in ["", "  ", "../Documents", "a/b", "a\\b", ".hidden", "CON", "nul.txt", "a:b", "line\nbreak", "..", "."] {
        assert_eq!(folder_name(bad), None, "{bad:?}");
    }
    assert_eq!(metadata_name(br#"{"name":"Curse of Strahd"}"#).as_deref(), Some("Curse of Strahd"));
    assert_eq!(metadata_name(br#"{"name":"../../Library"}"#), None);
    assert_eq!(metadata_name(br#"{"name":42}"#), None);
    assert_eq!(metadata_name(b"not json"), None);
}

#[test]
fn a_damaged_or_foreign_state_file_means_a_fresh_catch_up() {
    let dir = temp("state");
    let file = dir.join("room.json");
    let key = [3u8; 32];
    let entry = |path: &str| Entry { id: file_id(&key, path), seq: 4, hash: content_hash(b"x"), len: 1, gone: false };
    let good = State { seq: 9, files: [("NPCs/Vex.md".to_string(), entry("NPCs/Vex.md"))].into() };
    save_state(&file, &good).unwrap();
    assert_eq!(load_state(&file, &key), good);
    assert_eq!(load_state(&file, &[4u8; 32]), State::default(), "ids made with another key");
    fs::write(&file, "{not json").unwrap();
    assert_eq!(load_state(&file, &key), State::default());
    let evil = State { seq: 9, files: [("../escape.md".to_string(), entry("../escape.md"))].into() };
    fs::write(&file, serde_json::to_vec(&evil).unwrap()).unwrap();
    assert_eq!(load_state(&file, &key), State::default(), "a path that doesn't sync");
    assert_eq!(load_state(&dir.join("missing.json"), &key), State::default());
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "no temporary files left behind");
    fs::remove_dir_all(dir).unwrap();
}

// ---------- end to end ----------

const CREATE_KEY: &str = "test-create-key-0123456789abcdefghijklmnop";

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lorekeeper-sync-{name}-{}", random_id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Removes the test's folders and the server's database, also when a step fails.
struct Cleanup(Vec<PathBuf>);

impl Drop for Cleanup {
    fn drop(&mut self) {
        for p in &self.0 {
            let _ = fs::remove_dir_all(p);
            for suffix in ["", "-wal", "-shm"] {
                let _ = fs::remove_file(format!("{}{suffix}", p.display()));
            }
        }
    }
}

async fn start_server(db: &Path) -> (sync_server::AppState, String) {
    let config = sync_server::Config {
        db_path: db.to_path_buf(),
        addr: ([127, 0, 0, 1], 0).into(),
        writes_per_minute: 100_000,
        create_key: Some(CREATE_KEY.into()),
        ..sync_server::Config::default()
    };
    let state = sync_server::AppState::new(config).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = sync_server::router(state.clone()).into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (state, format!("http://{addr}"))
}

#[derive(Default)]
struct Rec {
    statuses: Mutex<Vec<Status>>,
    presence: Mutex<Vec<String>>,
    warnings: Mutex<Vec<String>>,
}

impl Sink for Rec {
    fn status(&self, s: &Status) {
        self.statuses.lock().unwrap().push(s.clone());
    }
    fn presence(&self, names: &[String]) {
        *self.presence.lock().unwrap() = names.to_vec();
    }
    fn warning(&self, text: &str) {
        self.warnings.lock().unwrap().push(text.into());
    }
    fn wrote(&self, _: &Path) {}
    fn changed(&self) {}
}

impl Rec {
    fn last(&self) -> Option<Status> {
        self.statuses.lock().unwrap().last().cloned()
    }
}

struct Peer {
    root: PathBuf,
    state_file: PathBuf,
    rec: Arc<Rec>,
    engine: Engine,
    task: JoinHandle<()>,
}

#[derive(Clone)]
struct Room {
    server: String,
    room: String,
    key: [u8; 32],
}

fn start(room: &Room, root: &Path, state_file: &Path, token: &str, name: &str) -> Peer {
    let cfg = Config {
        root: root.to_path_buf(),
        state_file: state_file.to_path_buf(),
        server: room.server.clone(),
        room: room.room.clone(),
        key: Zeroizing::new(room.key),
        token: Zeroizing::new(token.into()),
        name: name.into(),
    };
    let rec = Arc::new(Rec::default());
    let (engine, task) = engine(cfg, rec.clone());
    Peer { root: root.to_path_buf(), state_file: state_file.to_path_buf(), rec, engine, task: tokio::spawn(task) }
}

impl Peer {
    async fn stop(self) {
        self.engine.stop();
        self.task.await.unwrap();
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
        self.engine.poke();
    }

    fn read(&self, rel: &str) -> Option<String> {
        fs::read_to_string(self.root.join(rel)).ok()
    }

    /// Every file that syncs, with its content.
    fn tree(&self) -> BTreeMap<String, Vec<u8>> {
        let mut files = BTreeMap::new();
        walk(&self.root, "", &mut files);
        files.into_keys().map(|rel| (rel.clone(), fs::read(self.root.join(&rel)).unwrap())).collect()
    }

    fn conflict_copies(&self, dir: &str) -> Vec<String> {
        let names = fs::read_dir(self.root.join(dir)).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned());
        names.filter(|n| is_conflict_copy(n)).collect()
    }

    fn synced_hash(&self, key: &[u8; 32], rel: &str) -> Option<String> {
        load_state(&self.state_file, key).files.get(rel).filter(|e| !e.gone).map(|e| e.hash.clone())
    }
}

/// Waits for a condition (the engines work in the background); fails the test after 15 seconds.
async fn until(what: &str, cond: impl Fn() -> bool) {
    let start = Instant::now();
    while !cond() {
        assert!(start.elapsed() < Duration::from_secs(15), "timed out waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_players_sync_through_the_server() {
    let started = Instant::now();
    let base = temp("e2e");
    let db = base.with_extension("db");
    let _cleanup = Cleanup(vec![base.clone(), db.clone()]);
    let (owner_dir, member_dir, outside) = (base.join("owner"), base.join("member"), base.join("outside"));
    for d in [&owner_dir, &member_dir, &outside] {
        fs::create_dir_all(d).unwrap();
    }
    let (_server, url) = start_server(&db).await;

    // The owner's campaign, with things that must never leave it.
    let files = [
        ("PCs/Lorelei.md", "# Lorelei"), ("PCs/Syloth.md", "# Syloth"), ("NPCs/Vex.md", "# Vex\n"), ("Locations/Inn.md", "# Inn"),
        ("Templates/NPC.md", "# {{title}}"), (".obsidian/app.md", "{}"), ("notes.txt", "private"), ("Lore/run.sh", "rm -rf ~"),
        (METADATA, r#"{"name":"Curse of Strahd"}"#),
    ];
    for (rel, text) in files {
        fs::create_dir_all(owner_dir.join(rel).parent().unwrap()).unwrap();
        fs::write(owner_dir.join(rel), text).unwrap();
    }
    let portrait: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    fs::create_dir_all(owner_dir.join("Attachments")).unwrap();
    fs::write(owner_dir.join("Attachments/map.png"), &portrait).unwrap();

    // Share: the server wants its creation key.
    let u = url.clone();
    let refused = blocking(move || create_room(&u, None)).await.unwrap_err();
    assert_eq!((refused.status, refused.code.as_str()), (403, "create_key_required"));
    let u = url.clone();
    let created = blocking(move || create_room(&u, Some(CREATE_KEY))).await.unwrap();
    let room = Room { server: url.clone(), room: created.room.clone(), key: random_secret() };
    let owner_token = created.owner_token.clone();
    let owner = start(&room, &owner_dir, &base.join("owner-state.json"), &owner_token, "Lorelei");
    until("the owner's first upload", || owner.rec.last() == Some(Status::Synced)).await;

    // Invite: the link carries the key in its fragment; the player redeems it for a token of their own.
    let (u, r, t) = (url.clone(), room.room.clone(), owner_token.clone());
    let invite = blocking(move || create_invite(&u, &r, &t)).await.unwrap();
    let link = Invite { server: url.clone(), room: room.room.clone(), invite: invite.invite.clone(), key: room.key }.link();
    let parsed = Invite::parse(&link).unwrap();
    assert_eq!(parsed.key, room.key);
    let (u, r, i, m) = (parsed.server.clone(), parsed.room.clone(), parsed.invite.clone(), seal_member(&parsed.key, &parsed.room, ""));
    let member_token = blocking(move || redeem(&u, &r, &i, &m)).await.unwrap().token;
    let (u, r, i) = (url.clone(), room.room.clone(), invite.invite.clone());
    assert_eq!(blocking(move || redeem(&u, &r, &i, "")).await.unwrap_err().code, "invite_used");

    // Join: the campaign's name comes first, then everything downloads.
    let name = fetch_name(&url, &room.room, &room.key, &member_token).await;
    assert_eq!(name.as_deref(), Some("Curse of Strahd"));
    let member_state = base.join("member-state.json");
    let member = start(&room, &member_dir, &member_state, &member_token, "Syloth");
    until("the first download", || member.rec.last() == Some(Status::Synced)).await;
    assert_eq!(member.tree(), owner.tree());
    assert_eq!(fs::read(member_dir.join("Attachments/map.png")).unwrap(), portrait);
    assert_eq!(metadata_name(&fs::read(member_dir.join(METADATA)).unwrap()).as_deref(), Some("Curse of Strahd"));
    for private in ["Templates/NPC.md", ".obsidian/app.md", "notes.txt", "Lore/run.sh"] {
        assert!(!member_dir.join(private).exists(), "{private} must not sync");
    }

    // Presence: each sees the other's PC name, decrypted.
    until("presence", || {
        *owner.rec.presence.lock().unwrap() == ["Syloth"] && *member.rec.presence.lock().unwrap() == ["Lorelei"]
    })
    .await;

    // Everyone writes their own session file; both see both.
    owner.write("Sessions/Session 1/Lorelei.md", "- 20:01 we meet Vex");
    member.write("Sessions/Session 1/Syloth.md", "- 20:02 Vex lies");
    until("each other's session notes", || {
        member.read("Sessions/Session 1/Lorelei.md").as_deref() == Some("- 20:01 we meet Vex")
            && owner.read("Sessions/Session 1/Syloth.md").as_deref() == Some("- 20:02 Vex lies")
    })
    .await;

    // The member goes offline; both edit the same page. The owner's edit lands first.
    member.stop().await;
    owner.write("NPCs/Vex.md", "# Vex\nowner version\n");
    until("the owner's edit is stored", || owner.synced_hash(&room.key, "NPCs/Vex.md") == Some(content_hash(b"# Vex\nowner version\n"))).await;
    fs::write(member_dir.join("NPCs/Vex.md"), "# Vex\nmember version\n").unwrap();
    fs::create_dir_all(member_dir.join("Lore")).unwrap();
    fs::write(member_dir.join("Lore/Offline.md"), "written offline").unwrap();

    // Back online: the member catches up, keeps their version, and gets exactly one copy of the owner's.
    let member = start(&room, &member_dir, &member_state, &member_token, "Syloth");
    until("the offline note reaches the owner", || owner.read("Lore/Offline.md").as_deref() == Some("written offline")).await;
    until("the member's version wins", || owner.read("NPCs/Vex.md").as_deref() == Some("# Vex\nmember version\n")).await;
    assert_eq!(member.read("NPCs/Vex.md").as_deref(), Some("# Vex\nmember version\n"));
    let copies = member.conflict_copies("NPCs");
    assert_eq!(copies.len(), 1, "{copies:?}");
    assert_eq!(member.read(&format!("NPCs/{}", copies[0])).as_deref(), Some("# Vex\nowner version\n"));
    assert!(owner.conflict_copies("NPCs").is_empty());
    until("the member is synced", || member.rec.last() == Some(Status::Synced)).await;
    assert!(!owner.root.join(format!("NPCs/{}", copies[0])).exists(), "conflict copies don't sync");

    // A deletion propagates; the file goes to the campaign's .trash, not away.
    fs::remove_file(owner_dir.join("Locations/Inn.md")).unwrap();
    owner.engine.poke();
    until("the deletion", || !member_dir.join("Locations/Inn.md").exists()).await;
    assert_eq!(member.read(".trash/Locations/Inn.md").as_deref(), Some("# Inn"));
    assert!(member_dir.join("Locations").is_dir(), "top-level folders stay");

    // Deleting a page the other player changed meanwhile: theirs stays and comes back.
    member.stop().await;
    fs::write(member_dir.join("Lore/Offline.md"), "changed while it was deleted").unwrap();
    fs::remove_file(owner_dir.join("Lore/Offline.md")).unwrap();
    owner.engine.poke();
    until("the owner's delete is stored", || owner.synced_hash(&room.key, "Lore/Offline.md").is_none()).await;
    let member = start(&room, &member_dir, &member_state, &member_token, "Syloth");
    until("the changed page comes back", || owner.read("Lore/Offline.md").as_deref() == Some("changed while it was deleted")).await;
    assert_eq!(member.read("Lore/Offline.md").as_deref(), Some("changed while it was deleted"));

    // A rogue member (or a compromised server) sends hostile changes: nothing lands, and sync goes on.
    let (u, r, t) = (url.clone(), room.room.clone(), owner_token.clone());
    let rogue_invite = blocking(move || create_invite(&u, &r, &t)).await.unwrap().invite;
    let (u, r) = (url.clone(), room.room.clone());
    let rogue_token = blocking(move || redeem(&u, &r, &rogue_invite, "")).await.unwrap().token;
    rogue_puts(&room, &rogue_token).await;
    until("the good change after the hostile ones", || {
        member.read("NPCs/Good.md").as_deref() == Some("fine") && owner.read("NPCs/Good.md").as_deref() == Some("fine")
    })
    .await;
    for root in [&owner_dir, &member_dir] {
        assert!(!base.join("escape.md").exists() && !root.join("../escape.md").exists());
        for bad in ["NPCs/evil.sh", "NPCs/Other.md", "NPCs/Real.md", ".obsidian/plugins/x.md", "CON.md", "NPCs/garbage.md"] {
            assert!(!root.join(bad).exists(), "{bad}");
        }
    }
    // The server's own delete and a garbage overwrite (anyone with a token can send them, key or not) change
    // nothing here, and the party's version goes back up over them.
    for (who, peer) in [("owner", &owner), ("member", &member)] {
        assert_eq!(peer.read("NPCs/Vex.md").as_deref(), Some("# Vex\nmember version\n"), "{who}");
        assert_eq!(peer.read("PCs/Lorelei.md").as_deref(), Some("# Lorelei"), "{who}");
    }
    until("the party's versions are back on the server", || {
        let (vex, lorelei) = (content_hash(b"# Vex\nmember version\n"), content_hash(b"# Lorelei"));
        [&owner, &member].iter().all(|p| {
            p.synced_hash(&room.key, "NPCs/Vex.md") == Some(vex.clone()) && p.synced_hash(&room.key, "PCs/Lorelei.md") == Some(lorelei.clone())
        })
    })
    .await;
    let warnings = member.rec.warnings.lock().unwrap().clone();
    assert!(warnings.iter().any(|w| w.contains("decrypted")), "{warnings:?}");
    assert!(warnings.iter().any(|w| w.contains("Ignored a deletion")), "{warnings:?}");
    assert!(warnings.iter().any(|w| w.contains("kind Lorekeeper doesn't sync")), "{warnings:?}");
    assert!(warnings.iter().all(|w| !w.contains("NPCs") && !w.contains(".sh")), "warnings name no files: {warnings:?}");

    // Symlinks are never followed: not to upload a file from outside, not to write into a folder outside.
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        fs::write(outside.join("secret.md"), "outside the campaign").unwrap();
        symlink(outside.join("secret.md"), member_dir.join("NPCs/Link.md")).unwrap();
        symlink(&outside, member_dir.join("Lore/Elsewhere")).unwrap();
        symlink(&outside, owner_dir.join("Items")).unwrap();
        member.write("Items/Sword.md", "a sword");
        member.write("NPCs/Marker.md", "after the links");
        until("the marker", || owner.read("NPCs/Marker.md").is_some()).await;
        assert!(!owner_dir.join("NPCs/Link.md").exists(), "a symlinked file isn't uploaded");
        assert!(!owner_dir.join("Lore/Elsewhere").exists(), "a symlinked folder isn't uploaded");
        assert!(!outside.join("Sword.md").exists(), "nothing is written through a symlinked folder");
        assert!(owner.rec.warnings.lock().unwrap().iter().any(|w| w.contains("isn't a plain file")));
    }
    let _ = &outside;

    // The owner removes the player: their sync stops for good, their files stay.
    let (u, r, t) = (url.clone(), room.room.clone(), owner_token.clone());
    let players = blocking(move || members(&u, &r, &t)).await.unwrap();
    let names: Vec<String> = players.iter().map(|p| open_member(&room.key, &room.room, &p.member).unwrap()).collect();
    let syloth = players.iter().zip(&names).find(|(_, n)| *n == "Syloth").map(|(p, _)| p.member_id.clone()).unwrap();
    assert!(players.iter().zip(&names).any(|(p, n)| p.role == Role::Owner && n == "Lorelei"));
    let (u, r, t) = (url.clone(), room.room.clone(), owner_token.clone());
    blocking(move || remove_member(&u, &r, &t, &syloth)).await.unwrap();
    until("the removal reaches the player", || member.rec.last() == Some(Status::Removed)).await;
    tokio::time::timeout(Duration::from_secs(5), member.task).await.expect("a removed engine stops").unwrap();
    assert_eq!(fs::read_to_string(member_dir.join("NPCs/Vex.md")).unwrap(), "# Vex\nmember version\n");

    // Removed while offline: the next connection is refused, which also means removed.
    let again = start(&room, &member_dir, &member_state, &member_token, "Syloth");
    until("a revoked token is refused", || again.rec.last() == Some(Status::Removed)).await;
    owner.stop().await;
    eprintln!("end-to-end sync test took {:?}", started.elapsed());
}

/// A member token's socket that puts what a well-behaved client never would, then one good page.
async fn rogue_puts(room: &Room, token: &str) {
    let mut req = format!("{}/v1/rooms/{}/live", room.server.replace("http://", "ws://"), room.room).into_client_request().unwrap();
    req.headers_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    let send = |msg: ClientMessage| Message::Text(serde_json::to_string(&msg).unwrap().into());
    ws.send(send(ClientMessage::Hello { since: 0, member: String::new() })).await.unwrap();
    // The current version of every file, to aim the server-side delete and the garbage overwrite.
    let mut seqs = HashMap::new();
    while let Some(Ok(msg)) = ws.next().await {
        let Message::Text(t) = msg else { continue };
        if let Ok(ServerMessage::Changes { changes, more, .. }) = serde_json::from_str(t.as_str()) {
            seqs.extend(changes.into_iter().map(|c| (c.id, c.seq)));
            if !more {
                break;
            }
        }
    }
    let key = room.key;
    let sealed = |id: &str, path: &str, text: &str| {
        encode_blob(&seal(&key, &room.room, id, &FileContent { path: path.into(), content: text.as_bytes().to_vec(), ..Default::default() }))
    };
    let mut puts = Vec::new();
    for path in ["../escape.md", "NPCs/evil.sh", ".obsidian/plugins/x.md", "CON.md"] {
        let id = file_id(&key, path);
        puts.push((id.clone(), sealed(&id, path, "pwned")));
    }
    // A blob that claims another path than its id's.
    let other = file_id(&key, "NPCs/Other.md");
    puts.push((other.clone(), sealed(&other, "NPCs/Real.md", "pwned")));
    // Garbage that isn't a blob for this room at all.
    puts.push((file_id(&key, "NPCs/garbage.md"), encode_blob(&[7u8; 200])));
    // Garbage over a file everyone has, and the server's own delete of another.
    let lorelei = file_id(&key, "PCs/Lorelei.md");
    puts.push((lorelei.clone(), encode_blob(&[9u8; 200])));
    let good = file_id(&key, "NPCs/Good.md");
    puts.push((good.clone(), sealed(&good, "NPCs/Good.md", "fine")));
    let vex = file_id(&key, "NPCs/Vex.md");
    ws.send(send(ClientMessage::Delete { req: 99, base: seqs[&vex], id: vex })).await.unwrap();
    let last = puts.len() as u64 - 1;
    for (req, (id, blob)) in puts.into_iter().enumerate() {
        let base = seqs.get(&id).copied().unwrap_or(0);
        ws.send(send(ClientMessage::Put { req: req as u64, id, base, blob })).await.unwrap();
    }
    // Wait for the last ack so the writes are stored before the socket goes.
    while let Some(Ok(msg)) = ws.next().await {
        if let Message::Text(t) = msg {
            match serde_json::from_str(t.as_str()) {
                Ok(ServerMessage::Ack { req, .. }) if req == last => break,
                Ok(ServerMessage::Conflict { .. } | ServerMessage::Error { .. }) => panic!("the rogue's write was refused: {t}"),
                _ => {}
            }
        }
    }
    let _ = ws.close(None).await;
}
