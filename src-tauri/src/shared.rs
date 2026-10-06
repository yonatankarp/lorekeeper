//! Shared campaigns in the app: the room's secrets in the keychain, Share / Invite / Join / Players for the settings
//! window, and the sync engine (sync.rs) for the open campaign. Only the open campaign syncs; switching to another
//! shared campaign disconnects and connects that one, which catches up.
//!
//! Secrets (room key, token, the server's creation key) live only in the keychain. None of them goes into settings,
//! state files, events or error messages; invite links (which hold the key) are made only when the owner asks.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sync_protocol::{decode_secret, encode_secret, is_id, open_member, random_secret, Invite, Role, SECRET_LEN};
use tauri::{AppHandle, Emitter};
use zeroize::Zeroizing;

use crate::sync::{self, Status};
use crate::{author, config_dir, current_settings, emit_changed, library_dir, off_main, store_settings, watch, Settings, Sharing};

// ---------- the server and the keychain ----------

/// The sync server for new shared campaigns: the Sync server setting, else `LOREKEEPER_SYNC_SERVER` (for testing
/// against a local server), else the maintainer's.
pub(crate) fn server(s: &Settings) -> Result<String, String> {
    let url = match s.sync_server.trim() {
        "" => std::env::var("LOREKEEPER_SYNC_SERVER").unwrap_or_else(|_| sync::DEFAULT_SERVER.into()),
        url => url.to_string(),
    };
    sync_protocol::server_origin(&url).map_err(|e| format!("Sync server: {e}."))
}

#[derive(Serialize, Deserialize)]
struct RoomSecret {
    key: String,
    token: String,
}

fn room_entry(room: &str) -> Result<keyring::Entry, String> {
    crate::github::keychain(&format!("sync-room {room}"))
}

/// The creation key is kept per server origin and only ever sent to that server.
fn create_key_entry(server: &str) -> Result<keyring::Entry, String> {
    crate::github::keychain(&format!("sync-create-key {server}"))
}

fn save_room(room: &str, key: &[u8; SECRET_LEN], token: &str) -> Result<(), String> {
    let secret = Zeroizing::new(serde_json::to_vec(&RoomSecret { key: encode_secret(key), token: token.into() }).expect("serializes"));
    room_entry(room)?.set_secret(&secret).map_err(|e| format!("Couldn't save the campaign's key in the system's password storage: {e}"))
}

type Secrets = (Zeroizing<[u8; SECRET_LEN]>, Zeroizing<String>);

fn load_room(room: &str) -> Result<Secrets, String> {
    let bytes = Zeroizing::new(room_entry(room)?.get_secret().map_err(|e| match e {
        keyring::Error::NoEntry => "This campaign's key isn't on this computer. Join it again from an invite link.".to_string(),
        e => format!("Couldn't read the campaign's key: {e}"),
    })?);
    let secret: RoomSecret = serde_json::from_slice(&bytes).map_err(|_| "The campaign's saved key is damaged.".to_string())?;
    let key = Zeroizing::new(decode_secret(&secret.key).map_err(|_| "The campaign's saved key is damaged.".to_string())?);
    Ok((key, Zeroizing::new(secret.token)))
}

/// Forgets a room on this computer: its secrets and its sync state. The campaign's files stay.
fn forget_room(app: &AppHandle, room: &str) {
    if let Ok(entry) = room_entry(room) {
        let _ = entry.delete_credential();
    }
    if let Ok(file) = state_file(app, room) {
        let _ = fs::remove_file(file);
    }
}

fn state_file(app: &AppHandle, room: &str) -> Result<PathBuf, String> {
    Ok(config_dir(app).map_err(|e| e.to_string())?.join("sync").join(format!("{room}.json")))
}

// ---------- the engine for the open campaign ----------

struct Running {
    target: Target,
    engine: sync::Engine,
}

#[derive(Clone, PartialEq)]
struct Target {
    path: String,
    server: String,
    room: String,
    name: String,
}

static RUNNING: Mutex<Option<Running>> = Mutex::new(None);
/// Bumped by each restart, so a slow start that's been overtaken doesn't install itself.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// What a window shows for a campaign: its sync status, who else is online, and the last warning.
#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    path: String,
    status: Option<Status>,
    online: Vec<String>,
    warning: String,
}

static SNAPSHOTS: Mutex<BTreeMap<String, Snapshot>> = Mutex::new(BTreeMap::new());

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn update(app: &AppHandle, path: &str, f: impl FnOnce(&mut Snapshot)) {
    let snap = {
        let mut all = lock(&SNAPSHOTS);
        let snap = all.entry(path.to_string()).or_insert_with(|| Snapshot { path: path.into(), ..Snapshot::default() });
        f(snap);
        snap.clone()
    };
    let _ = app.emit("sync-status", snap);
}

/// The open campaign's room, when it's shared, has one, and wasn't left.
fn target(s: &Settings) -> Option<Target> {
    let sh = s.sharing.get(&s.vault_path).filter(|sh| sh.shared && !sh.room.is_empty() && !sh.removed)?;
    let name = author(s).ok().flatten().unwrap_or_default();
    Some(Target { path: s.vault_path.clone(), server: sh.server.clone(), room: sh.room.clone(), name })
}

/// Makes the running engine match the settings: none, the same one, or the open campaign's (with your PC's name,
/// which presence shows). Cheap when nothing changed; called after every settings change.
pub(crate) fn restart(app: &AppHandle) {
    let want = target(&current_settings(app));
    let mut running = lock(&RUNNING);
    if running.as_ref().map(|r| &r.target) == want.as_ref() {
        return;
    }
    if let Some(old) = running.take() {
        old.engine.stop();
        update(app, &old.target.path, |s| s.online.clear());
    }
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let Some(want) = want else { return };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let room = want.room.clone();
        let loaded = tauri::async_runtime::spawn_blocking(move || load_room(&room)).await.map_err(|e| e.to_string()).and_then(|r| r);
        let path = want.path.clone();
        let (key, token) = match loaded {
            Ok(secrets) => secrets,
            Err(e) => return update(&app, &path, |s| s.status = Some(Status::Stopped { reason: e })),
        };
        let state_file = match state_file(&app, &want.room) {
            Ok(f) => f,
            Err(e) => return update(&app, &path, |s| s.status = Some(Status::Stopped { reason: e })),
        };
        let cfg = sync::Config {
            root: PathBuf::from(&want.path),
            state_file,
            server: want.server.clone(),
            room: want.room.clone(),
            key,
            token,
            name: want.name.clone(),
        };
        let mut running = lock(&RUNNING);
        if GENERATION.load(Ordering::SeqCst) != generation {
            return; // settings changed again meanwhile
        }
        let (engine, task) = sync::engine(cfg, Arc::new(AppSink { app: app.clone(), path: path.clone() }));
        tauri::async_runtime::spawn(task);
        *running = Some(Running { target: want, engine });
    });
}

/// Local files may have changed (watch.rs saw something): the engine looks and pushes.
pub(crate) fn poke() {
    if let Some(r) = lock(&RUNNING).as_ref() {
        r.engine.poke();
    }
}

struct AppSink {
    app: AppHandle,
    path: String,
}

impl sync::Sink for AppSink {
    fn status(&self, status: &Status) {
        update(&self.app, &self.path, |s| {
            s.status = Some(status.clone());
            if matches!(status, Status::Offline | Status::Removed | Status::Stopped { .. }) {
                s.online.clear();
            }
            if *status == Status::Synced {
                s.warning.clear();
            }
        });
        if *status == Status::Removed {
            removed(&self.app, &self.path);
        }
    }

    fn presence(&self, names: &[String]) {
        update(&self.app, &self.path, |s| s.online = names.to_vec());
    }

    fn warning(&self, text: &str) {
        update(&self.app, &self.path, |s| s.warning = text.into());
    }

    fn wrote(&self, path: &Path) {
        watch::wrote(path);
    }

    fn changed(&self) {
        emit_changed(&self.app);
    }
}

/// The owner removed you: the campaign stops syncing for good (its files stay), and its token is deleted.
fn removed(app: &AppHandle, path: &str) {
    let mut s = current_settings(app);
    let Some(sh) = s.sharing.get_mut(path) else { return };
    let room = std::mem::take(&mut sh.room);
    sh.removed = true;
    let _ = store_settings(app, &s);
    let app = app.clone();
    std::thread::spawn(move || forget_room(&app, &room));
}

/// Keeps the webview from changing a campaign's room (only Share and Join set it), and leaves the room of a joined
/// campaign you remove from Lorekeeper. The owner's secrets stay: they're the only way to invite or remove players.
pub(crate) fn guard_settings(app: &AppHandle, old: &Settings, new: &mut Settings) {
    for room in guard(old, new) {
        let app = app.clone();
        std::thread::spawn(move || forget_room(&app, &room));
    }
}

/// See guard_settings; returns the rooms to forget. A save from a window that hadn't heard of a campaign yet (no
/// sharing entry for it) can't drop or leave it: its entry and its place in the list come back.
fn guard(old: &Settings, new: &mut Settings) -> Vec<String> {
    for (path, sh) in new.sharing.iter_mut() {
        let o = old.sharing.get(path).cloned().unwrap_or_default();
        (sh.server, sh.room, sh.role, sh.removed) = (o.server, o.room, o.role, o.removed);
    }
    let mut forget = Vec::new();
    let dropped: Vec<&String> = old.campaigns.iter().filter(|p| !new.campaigns.contains(p)).collect();
    for path in dropped {
        match new.sharing.get_mut(path) {
            Some(sh) if sh.role == "member" && !sh.room.is_empty() => forget.push(std::mem::take(&mut sh.room)),
            Some(_) => {}
            None if old.sharing.get(path).is_some_and(|sh| !sh.room.is_empty()) => new.campaigns.push(path.clone()),
            None => {}
        }
    }
    for (path, sh) in &old.sharing {
        new.sharing.entry(path.clone()).or_insert_with(|| sh.clone());
    }
    forget
}

// ---------- commands for the windows ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Info {
    /// The server new shared campaigns go to, or why the setting is wrong.
    server: Result<String, String>,
    default_server: &'static str,
    statuses: Vec<Snapshot>,
}

#[tauri::command]
pub(crate) fn sync_info(app: AppHandle) -> Info {
    Info { server: server(&current_settings(&app)), default_server: sync::DEFAULT_SERVER, statuses: lock(&SNAPSHOTS).values().cloned().collect() }
}

/// One of your campaigns and how it's shared; every command checks the path it's given against your settings.
fn campaign(app: &AppHandle, path: &str) -> Result<(Settings, Sharing), String> {
    let s = current_settings(app);
    if !s.campaigns.iter().any(|c| c == path) {
        return Err("That isn't one of your campaigns.".into());
    }
    let sh = s.sharing.get(path).cloned().unwrap_or_default();
    Ok((s, sh))
}

/// A campaign you share: (server, room, owner token).
fn owned(app: &AppHandle, path: &str) -> Result<(String, String, Secrets), String> {
    let (_, sh) = campaign(app, path)?;
    if sh.role != "owner" || sh.room.is_empty() {
        return Err("Only the player who shared this campaign can do that.".into());
    }
    let secrets = load_room(&sh.room)?;
    Ok((sh.server, sh.room, secrets))
}

/// Share: makes the campaign's room on the sync server, keeps its key, writes the campaign's name for joiners, and
/// starts syncing (the first sync uploads everything). Err("create_key_required") asks the window for the server's
/// creation key, which is then remembered for that server only.
#[tauri::command]
pub(crate) async fn sync_share(app: AppHandle, path: String, create_key: Option<String>) -> Result<(), String> {
    let (s, sh) = campaign(&app, &path)?;
    if !sh.shared {
        return Err("Turn on Shared with my party first.".into());
    }
    if !sh.room.is_empty() {
        return Err("This campaign is already shared.".into());
    }
    let server = server(&s)?;
    let given = create_key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
    if given.as_ref().is_some_and(|k| k.len() > 512 || k.chars().any(|c| c.is_control() || !c.is_ascii())) {
        return Err("That isn't a creation key.".into());
    }
    let name = crate::backup::backup_name(&s, &path);
    let srv = server.clone();
    let room = off_main(move || {
        let stored = || create_key_entry(&srv).ok()?.get_password().ok().map(Zeroizing::new);
        let key = given.clone().map(Zeroizing::new).or_else(stored);
        let created = match sync::create_room(&srv, key.as_deref().map(|k| k.as_str())) {
            Ok(r) => r,
            Err(e) if e.code == "create_key_required" && given.is_some() => return Err("That creation key isn't right.".into()),
            Err(e) if e.code == "create_key_required" => return Err("create_key_required".into()),
            Err(e) => return Err(e.to_string()),
        };
        if let Some(k) = &given {
            create_key_entry(&srv)?.set_password(k).map_err(|e| format!("Couldn't save the creation key: {e}"))?;
        }
        let key = Zeroizing::new(random_secret());
        save_room(&created.room, &key, &created.owner_token)?;
        Ok(created.room)
    })
    .await?;
    let metadata = Path::new(&path).join(sync::METADATA);
    fs::create_dir_all(metadata.parent().expect("has a folder")).map_err(|e| e.to_string())?;
    fs::write(&metadata, serde_json::json!({ "name": name }).to_string()).map_err(|e| e.to_string())?;
    let mut s = current_settings(&app);
    s.sharing.insert(path.clone(), Sharing { server, room, role: "owner".into(), removed: false, ..s.sharing.get(&path).cloned().unwrap_or_default() });
    store_settings(&app, &s)
}

/// Invite a player (or several): one-time links, each made now and shown once. The key is in the part after #.
#[tauri::command]
pub(crate) async fn sync_invite(app: AppHandle, path: String, count: u32) -> Result<Vec<String>, String> {
    if !(1..=10).contains(&count) {
        return Err("Make between 1 and 10 invites at a time.".into());
    }
    let (server, room, (key, token)) = owned(&app, &path)?;
    off_main(move || {
        let mut links = Vec::new();
        for _ in 0..count {
            let invite = sync::create_invite(&server, &room, &token).map_err(|e| e.to_string())?;
            links.push(Invite { server: server.clone(), room: room.clone(), invite: invite.invite, key: *key }.link());
        }
        Ok(links)
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PendingInvite {
    invite: String,
    expires: i64,
}

#[tauri::command]
pub(crate) async fn sync_invites(app: AppHandle, path: String) -> Result<Vec<PendingInvite>, String> {
    let (server, room, (_, token)) = owned(&app, &path)?;
    let list = off_main(move || sync::list_invites(&server, &room, &token).map_err(|e| e.to_string())).await?;
    Ok(list.into_iter().filter(|i| is_id(&i.invite)).map(|i| PendingInvite { invite: i.invite, expires: i.expires }).collect())
}

#[tauri::command]
pub(crate) async fn sync_cancel_invite(app: AppHandle, path: String, invite: String) -> Result<(), String> {
    if !is_id(&invite) {
        return Err("That isn't an invite.".into());
    }
    let (server, room, (_, token)) = owned(&app, &path)?;
    off_main(move || sync::cancel_invite(&server, &room, &token, &invite).map_err(|e| e.to_string())).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Player {
    member_id: String,
    /// Their PC's name, decrypted; "" when they haven't picked one yet.
    name: String,
    owner: bool,
    created: i64,
    last_seen: Option<i64>,
}

#[tauri::command]
pub(crate) async fn sync_members(app: AppHandle, path: String) -> Result<Vec<Player>, String> {
    let (server, room, (key, token)) = owned(&app, &path)?;
    let list = off_main({
        let room = room.clone();
        move || sync::members(&server, &room, &token).map_err(|e| e.to_string())
    })
    .await?;
    Ok(list
        .into_iter()
        .filter(|m| is_id(&m.member_id))
        .map(|m| Player {
            name: open_member(&key, &room, &m.member).unwrap_or_default().chars().filter(|c| !c.is_control()).take(60).collect(),
            owner: m.role == Role::Owner,
            member_id: m.member_id,
            created: m.created,
            last_seen: m.last_seen,
        })
        .collect())
}

#[tauri::command]
pub(crate) async fn sync_remove_member(app: AppHandle, path: String, member_id: String) -> Result<(), String> {
    if !is_id(&member_id) {
        return Err("That isn't a player.".into());
    }
    let (server, room, (_, token)) = owned(&app, &path)?;
    off_main(move || sync::remove_member(&server, &room, &token, &member_id).map_err(|e| e.to_string())).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InviteCheck {
    /// The server the link points at, for the player to confirm before joining.
    server: String,
    is_default: bool,
}

/// Reads a pasted invite link without using it: which server it's for.
#[tauri::command]
pub(crate) fn sync_check_invite(link: String) -> Result<InviteCheck, String> {
    let invite = Invite::parse(&link).map_err(|e| format!("That isn't a Lorekeeper invite link ({e})."))?;
    Ok(InviteCheck { is_default: invite.server == sync::DEFAULT_SERVER, server: invite.server })
}

/// Join a shared campaign: redeems the invite for a token of your own, makes the campaign's folder in the Lorekeeper
/// folder (named after the campaign, " 2" and so on when that's taken) and adds it. The window then opens it, which
/// starts the download, and asks which PC you play. Returns the new folder.
#[tauri::command]
pub(crate) async fn sync_join(app: AppHandle, link: String) -> Result<String, String> {
    let invite = Invite::parse(&link).map_err(|e| format!("That isn't a Lorekeeper invite link ({e})."))?;
    let s = current_settings(&app);
    if s.sharing.values().any(|sh| sh.room == invite.room) {
        return Err("You're already in this campaign.".into());
    }
    let (server, room, key) = (invite.server.clone(), invite.room.clone(), Zeroizing::new(invite.key));
    let token = off_main({
        let (server, room, invite_id, key) = (server.clone(), room.clone(), invite.invite.clone(), key.clone());
        move || {
            let redeemed = sync::redeem(&server, &room, &invite_id, "").map_err(|e| e.to_string())?;
            save_room(&room, &key, &redeemed.token)?;
            Ok(Zeroizing::new(redeemed.token))
        }
    })
    .await?;
    drop(invite);
    let name = sync::fetch_name(&server, &room, &key, &token).await.unwrap_or_else(|| "Shared campaign".into());
    let library = library_dir(&app);
    let s = current_settings(&app);
    // The first free folder whose name also passes the campaign checks (names and backup names must differ).
    let with = |p: &Path| {
        let mut s = s.clone();
        let path = p.to_string_lossy().into_owned();
        s.campaigns.push(path.clone());
        s.sharing.insert(path, Sharing { shared: true, server: server.clone(), room: room.clone(), role: "member".into(), ..Sharing::default() });
        s
    };
    let s = (1..=100)
        .map(|n| library.join(if n == 1 { name.clone() } else { format!("{name} {n}") }))
        .filter(|p| !p.exists())
        .map(|p| with(&p))
        .find(|s| crate::backup::check_campaigns(s).is_ok())
        .ok_or("Couldn't find a free folder name for the campaign.")?;
    let path = s.campaigns.last().expect("just added").clone();
    fs::create_dir_all(&path).map_err(|e| format!("Couldn't make the campaign's folder: {e}"))?;
    store_settings(&app, &s)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synced(role: &str, room: &str) -> Sharing {
        Sharing { shared: true, server: "https://lorekeeper.yonatankarp.com".into(), room: room.into(), role: role.into(), ..Sharing::default() }
    }

    #[test]
    fn the_settings_window_cant_redirect_or_drop_a_room() {
        let old = Settings {
            campaigns: vec!["/a".into(), "/b".into(), "/c".into()],
            sharing: [("/a".into(), synced("owner", "rooma")), ("/b".into(), synced("member", "roomb")), ("/c".into(), synced("member", "roomc"))].into(),
            ..Settings::default()
        };
        // A window tries to point a room at another server, take over another room, and make itself the owner.
        let mut new = old.clone();
        new.sharing.insert("/a".into(), Sharing { server: "https://evil.example".into(), room: "other".into(), ..synced("member", "") });
        new.sharing.insert("/b".into(), Sharing { role: "owner".into(), removed: true, me: "PCs/Arn.md".into(), ..synced("member", "roomb") });
        assert!(guard(&old, &mut new).is_empty());
        assert_eq!(new.sharing["/a"], synced("owner", "rooma"));
        assert_eq!(new.sharing["/b"], Sharing { me: "PCs/Arn.md".into(), ..synced("member", "roomb") }, "only shared and me are the window's");

        // Removing a joined campaign leaves its room; removing your own keeps it (the owner token can't be replaced).
        let mut new = Settings { campaigns: vec!["/c".into()], ..old.clone() };
        assert_eq!(guard(&old, &mut new), ["roomb"]);
        assert_eq!(new.sharing["/b"].room, "");
        assert_eq!(new.sharing["/a"].room, "rooma");

        // A window that hadn't heard of a campaign (no entry for it) can't drop it or leave it.
        let mut stale = Settings { campaigns: vec!["/a".into(), "/b".into()], sharing: old.sharing.clone(), ..old.clone() };
        stale.sharing.remove("/c");
        assert!(guard(&old, &mut stale).is_empty());
        assert!(stale.campaigns.contains(&"/c".to_string()));
        assert_eq!(stale.sharing["/c"], synced("member", "roomc"));
    }

    #[test]
    fn a_pasted_invite_says_which_server_its_for() {
        let link = |server: &str| Invite { server: server.into(), room: sync_protocol::random_id(), invite: sync_protocol::random_id(), key: [1; 32] }.link();
        let default = sync_check_invite(link(sync::DEFAULT_SERVER)).unwrap();
        assert!(default.is_default);
        let other = sync_check_invite(link("https://sync.example.org")).unwrap();
        assert_eq!((other.server.as_str(), other.is_default), ("https://sync.example.org", false));
        assert!(sync_check_invite(link("https://lorekeeper.yonatankarp.com.evil.example")).is_ok_and(|c| !c.is_default));
        assert!(sync_check_invite("https://example.org/not-an-invite".into()).is_err());
    }
}
