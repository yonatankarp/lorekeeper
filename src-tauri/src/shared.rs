//! Shared campaigns in the app: the room's secrets in the keychain, Share / Invite / Join / Players for the settings
//! window, and a sync engine (sync.rs) for each shared campaign, open or not (at most MAX_ENGINES at once, the open one
//! first). Switching campaigns reconnects nothing.
//!
//! Secrets (room key, token, the server's creation key) live only in the keychain. None of them goes into settings,
//! state files, events or error messages; invite links (which hold the key) are made only when the owner asks.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sync_protocol::{decode_secret, encode_secret, is_id, open_member, random_secret, Invite, Role, SECRET_LEN};
use tauri::{AppHandle, Emitter};
use zeroize::Zeroizing;

use crate::sync::{self, Access, Status};
use crate::{author, config_dir, current_settings, emit_changed, library_dir, off_main, store_settings, watch, Settings, Sharing, DM_ME};

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
    let encoded = Zeroizing::new(secret.key);
    let key = Zeroizing::new(decode_secret(&encoded).map_err(|_| "The campaign's saved key is damaged.".to_string())?);
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

// ---------- an engine for each shared campaign ----------

/// Shared campaigns that sync at once, the open one first; the rest wait (Status::Queued) until one stops or you open
/// them. Each holds a connection: the server allows 64 in all.
const MAX_ENGINES: usize = 8;

/// Becomes true when an engine's task ended (its last state save done), or its start gave up.
type Done = tokio::sync::watch::Receiver<bool>;

struct Running {
    target: Target,
    /// None while its start loads the room's secrets.
    engine: Option<sync::Engine>,
    done: Done,
    /// Tells this start from a later one for the same campaign: a start that was overtaken doesn't install itself.
    start: u64,
}

#[derive(Clone, PartialEq, Debug)]
struct Target {
    path: String,
    server: String,
    room: String,
    name: String,
    /// Your other campaigns inside this one's folder (a campaign kept in the Lorekeeper folder itself holds every
    /// campaign made or joined since): their files are theirs, never this room's (see sync::Config::nested).
    nested: Vec<String>,
}

/// Campaign folder -> its engine.
static RUNNING: Mutex<BTreeMap<String, Running>> = Mutex::new(BTreeMap::new());
/// Campaign folder -> its last engine that was told to stop: the next start for that folder waits for it to end, so two
/// engines never share a state file, even when a stop gave up waiting (stop_for) or one stopped meanwhile.
static STOPPING: Mutex<BTreeMap<String, Done>> = Mutex::new(BTreeMap::new());
static STARTS: AtomicU64 = AtomicU64::new(0);
/// Held around every read-modify-write of the settings (here and in lib.rs): engines report at the same moments (each
/// one's first access message), and two saves at once would lose one's change. Never held across an await.
pub(crate) static EDIT: Mutex<()> = Mutex::new(());

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

/// The campaigns that sync: shared, with a room, not left, and in your list, the open one first, then in list order, at
/// most MAX_ENGINES; and the ones past that, which wait. Each with your PC's name in it, which presence shows.
fn targets(s: &Settings) -> (Vec<Target>, Vec<String>) {
    let mut paths: Vec<&String> = s.campaigns.iter().collect();
    paths.sort_by_key(|p| **p != s.vault_path); // stable: the rest keep their order
    paths.dedup();
    let mut each = s.clone();
    let mut all: Vec<Target> = paths
        .into_iter()
        .filter_map(|path| {
            let sh = s.sharing.get(path).filter(|sh| sh.shared && !sh.room.is_empty() && !sh.removed)?;
            each.vault_path = path.clone(); // author() reads the open campaign's
            let name = author(&each).ok().flatten().unwrap_or_default();
            // Also a campaign removed from the list (its sharing entry and backup name stay): its folder is still there.
            let nested = s
                .campaigns
                .iter()
                .chain(s.sharing.keys())
                .chain(s.backup_names.keys())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .filter_map(|c| Path::new(c).strip_prefix(path).ok().filter(|rel| !rel.as_os_str().is_empty()))
                .map(|rel| rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"))
                .collect();
            Some(Target { path: path.clone(), server: sh.server.clone(), room: sh.room.clone(), name, nested })
        })
        .collect();
    let waiting = all.split_off(all.len().min(MAX_ENGINES)).into_iter().map(|t| t.path).collect();
    (all, waiting)
}

/// Resolves once an engine's task ended (or its start gave up).
async fn finished(mut done: Done) {
    let _ = done.wait_for(|d| *d).await;
}

/// Makes the running engines match the settings: each campaign that syncs gets one (with your PC's name, which presence
/// shows), any other stops. Cheap when nothing changed; called after every settings change.
pub(crate) fn restart(app: &AppHandle) {
    let (want, waiting) = targets(&current_settings(app));
    let mut running = lock(&RUNNING);
    let stale: Vec<String> = running.iter().filter(|(_, r)| !want.contains(&r.target)).map(|(p, _)| p.clone()).collect();
    for path in stale {
        let old = running.remove(&path).expect("listed");
        if let Some(engine) = &old.engine {
            engine.stop();
        }
        lock(&STOPPING).insert(path.clone(), old.done);
        update(app, &path, |s| s.online.clear());
    }
    for want in want {
        if !running.contains_key(&want.path) {
            let before = lock(&STOPPING).remove(&want.path);
            start(app, &mut running, want, before);
        }
    }
    drop(running);
    for path in waiting {
        update(app, &path, |s| {
            s.status = Some(Status::Queued);
            s.online.clear();
        });
    }
}

/// Starts a campaign's engine: loads its secrets, then (unless settings changed meanwhile) runs it. `before`, the same
/// campaign's engine that's stopping, ends first, so two engines never share a state file.
fn start(app: &AppHandle, running: &mut BTreeMap<String, Running>, want: Target, before: Option<Done>) {
    let start = STARTS.fetch_add(1, Ordering::SeqCst) + 1;
    let (done_tx, done) = tokio::sync::watch::channel(false);
    running.insert(want.path.clone(), Running { target: want.clone(), engine: None, done, start });
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Some(before) = before {
            finished(before).await;
        }
        let path = want.path.clone();
        // A start that fails is forgotten, so the next settings change tries again.
        let fail = |e: String| {
            let mut running = lock(&RUNNING);
            if running.get(&path).is_some_and(|r| r.start == start) {
                running.remove(&path);
                drop(running);
                update(&app, &path, |s| s.status = Some(Status::Stopped { reason: e }));
            }
        };
        let room = want.room.clone();
        let loaded = tauri::async_runtime::spawn_blocking(move || load_room(&room)).await.map_err(|e| e.to_string()).and_then(|r| r);
        let (key, token) = match loaded {
            Ok(secrets) => secrets,
            Err(e) => return fail(e),
        };
        let state_file = match state_file(&app, &want.room) {
            Ok(f) => f,
            Err(e) => return fail(e),
        };
        let cfg = sync::Config {
            root: PathBuf::from(&want.path),
            state_file,
            server: want.server.clone(),
            room: want.room.clone(),
            key,
            token,
            name: want.name.clone(),
            nested: want.nested.clone(),
        };
        let task = {
            let mut running = lock(&RUNNING);
            match running.get_mut(&path) {
                Some(r) if r.start == start => {
                    let (engine, task) = sync::engine(cfg, Arc::new(AppSink { app: app.clone(), path: path.clone() }));
                    r.engine = Some(engine);
                    task
                }
                _ => return, // settings changed again meanwhile
            }
        };
        task.await;
        let _ = done_tx.send(true);
    });
}

/// Stops a campaign's engine, if it runs (or is starting), and waits until it ended (it saves its state as it stops),
/// so it can't see the server close the room, or write its state, after the caller's cleanup. False when it was still
/// running after 10 seconds (a big upload); a later start for the campaign still waits for it (STOPPING). The next
/// restart starts it again if the settings still want it, which is how a failed Stop sharing, Leave or Delete campaign
/// resumes.
// ponytail: a restart from another settings change while the caller waits on the server starts the engine again (once
// this one ended); it then sees the room go (4003 or 401) and stops with a message, and the caller's cleanup still runs.
pub(crate) async fn stop_for(path: &str) -> bool {
    let removed = lock(&RUNNING).remove(path);
    if let Some(r) = removed {
        if let Some(engine) = &r.engine {
            engine.stop();
        }
        // A start still loading finds itself gone and never runs; its sender drops, which ends the wait too.
        lock(&STOPPING).insert(path.to_string(), r.done);
    }
    stopped(path, Duration::from_secs(10)).await
}

/// Waits, at most `wait`, for the campaign's last stopped engine to end; true when it did (or there was none).
async fn stopped(path: &str, wait: Duration) -> bool {
    let done = lock(&STOPPING).get(path).cloned();
    match done {
        Some(done) => tokio::time::timeout(wait, finished(done)).await.is_ok(),
        None => true,
    }
}

/// A campaign's local files may have changed (watch.rs saw something): its engine looks and pushes.
pub(crate) fn poke(path: &Path) {
    if let Some(engine) = lock(&RUNNING).get(path.to_string_lossy().as_ref()).and_then(|r| r.engine.as_ref()) {
        engine.poke();
    }
}

/// The campaign folders that sync now, for watch.rs.
pub(crate) fn synced() -> Vec<PathBuf> {
    lock(&RUNNING).keys().map(PathBuf::from).collect()
}

struct AppSink {
    app: AppHandle,
    path: String,
}

impl sync::Sink for AppSink {
    fn status(&self, status: &Status) {
        let mut status = status.clone();
        let mut left = None;
        let edit = matches!(status, Status::Removed | Status::Replaced | Status::Deleted).then(|| lock(&EDIT));
        if edit.is_some() {
            let mut s = current_settings(&self.app);
            match leave(&mut s, &self.path, &status) {
                Some(room) => {
                    if status == Status::Deleted {
                        let name = crate::backup::backup_name(&s, &self.path);
                        crate::notify(&self.app, "Campaign no longer shared", &format!("The owner stopped sharing {name}. Your copy stays on this computer."));
                    }
                    left = Some((s, room));
                }
                None if status == Status::Deleted => status = Status::Stopped { reason: OWNER_DELETED.into() },
                None => status = Status::Stopped { reason: OWNER_REFUSED.into() },
            }
        }
        let status = &status;
        update(&self.app, &self.path, |s| {
            s.status = Some(status.clone());
            if matches!(status, Status::Offline | Status::Removed | Status::Replaced | Status::Deleted | Status::Stopped { .. }) {
                s.online.clear();
            }
            if *status == Status::Synced {
                s.warning.clear();
            }
        });
        if let Some((s, room)) = left {
            let _ = store_settings(&self.app, &s);
            let app = self.app.clone();
            std::thread::spawn(move || forget_room(&app, &room)); // the engine saved its state before saying so
        }
        drop(edit);
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
        // Only the open campaign's window and backup care; another campaign's changes show when you open it.
        if crate::notes_dir(&self.app) == Path::new(&self.path) {
            emit_changed(&self.app);
        }
    }

    fn access(&self, access: &Access) {
        let _edit = lock(&EDIT);
        let mut s = current_settings(&self.app);
        if note_access(&mut s, &self.path, access) {
            let _ = store_settings(&self.app, &s);
        }
    }
}

/// Keeps what the server said about you in the campaign's settings; true when that changed anything. A change of the
/// private-notes setting raises the one-time notice for everyone but the owner (who made it).
fn note_access(s: &mut Settings, path: &str, access: &Access) -> bool {
    let Some(sh) = s.sharing.get_mut(path) else { return false };
    let picks_again = a_player_plays_a_character(sh, access);
    if sh.access.as_ref() == Some(access) {
        return picks_again;
    }
    let was = sh.access.as_ref().map(|a| a.dm_reads_private);
    if was.is_some_and(|was| was != access.dm_reads_private) && access.role != Role::Owner {
        sh.private_notice = true;
    }
    dm_plays_no_character(sh, access);
    sh.access = Some(access.clone());
    true
}

/// Joined from a DM's invite and no character picked yet: I play is "I'm the DM (no character)", so Lorekeeper
/// doesn't ask. Your notes then go to DM.md and presence shows "DM"; you can still pick a character in Party….
fn dm_plays_no_character(sh: &mut Sharing, access: &Access) {
    if sh.role == "member" && sh.me.is_empty() && access.role == Role::Dm {
        sh.me = DM_ME.into();
    }
}

/// A player with I play "I'm the DM (no character)" (picked before Lorekeeper offered it only to the owner and DMs,
/// or kept from when they were a DM) would file their notes as the DM's: they pick their character again instead.
/// Checked on every access message, unchanged ones too, so a choice from before is cleared at the next connect.
fn a_player_plays_a_character(sh: &mut Sharing, access: &Access) -> bool {
    let dm = sh.role == "member" && sh.me == DM_ME && access.role == Role::Player;
    if dm {
        sh.me.clear();
    }
    dm
}

const OWNER_REFUSED: &str =
    "The sync server doesn't accept this campaign's owner sign-in anymore. The campaign's key stays on this computer.";

const OWNER_DELETED: &str = "The sync server says this campaign was deleted. The campaign's key stays on this computer.";

/// The server refused the campaign's token (or closed with "removed", "replaced" or "deleted"). A member was removed
/// by the owner, `Replaced` by a re-invite redeemed on another computer, or the owner stopped sharing (`Deleted`): the
/// campaign stops syncing here for good (its files stay) and Some(room) is to be forgotten. An owner can't be removed,
/// and stops sharing from this computer, so for them it's a server problem (or a hostile one): None, and their secrets
/// stay, the only way to invite or remove players.
fn leave(s: &mut Settings, path: &str, why: &Status) -> Option<String> {
    // Not one made local meanwhile (Leave or Stop sharing while a stopping engine still ran): nothing to leave.
    let sh = s.sharing.get_mut(path).filter(|sh| sh.role != "owner" && !sh.room.is_empty())?;
    sh.removed = true;
    sh.replaced = *why == Status::Replaced;
    sh.unshared = *why == Status::Deleted;
    Some(std::mem::take(&mut sh.room))
}

/// Forgets the rooms guard_settings dropped (the token and state of a joined campaign you removed), and the DM copies in
/// their folders. Only once the settings without them were stored: a save that failed keeps the campaign syncing.
pub(crate) fn forget_rooms(app: &AppHandle, rooms: Vec<(String, String)>) {
    for (path, room) in rooms {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            // Storing the settings stopped its engine. Its last state save must not bring back the state file forgotten
            // here, nor its last change a DM copy dropped here, so this waits for it to end (every engine ends soon after
            // Stop: its connects and sends are bounded).
            stopped(&path, Duration::MAX).await;
            let _ = tauri::async_runtime::spawn_blocking(move || {
                forget_room(&app, &room);
                sync::drop_dm_copies(Path::new(&path)); // other members' private notes aren't yours to keep
            })
            .await;
        });
    }
}

/// Keeps the webview from changing a campaign's room (only Share and Join set it), and leaves the room of a joined
/// campaign you remove from Lorekeeper: returns the (campaign, room) pairs for forget_rooms, once the settings are stored.
/// The owner's secrets stay: they're the only way to invite or remove players. A save from a window that hadn't heard of a
/// campaign yet (no sharing entry for it) can't drop or leave it: its entry and its place in the list come back.
/// Only the engine sets what the server said (access); a window can only clear the notice it raised.
pub(crate) fn guard_settings(old: &Settings, new: &mut Settings) -> Vec<(String, String)> {
    for (path, sh) in new.sharing.iter_mut() {
        let o = old.sharing.get(path).cloned().unwrap_or_default();
        (sh.server, sh.room, sh.role, sh.removed, sh.replaced, sh.unshared, sh.access) =
            (o.server, o.room, o.role, o.removed, o.replaced, o.unshared, o.access);
        sh.private_notice &= o.private_notice;
    }
    let mut forget = Vec::new();
    let dropped: Vec<&String> = old.campaigns.iter().filter(|p| !new.campaigns.contains(p)).collect();
    for path in dropped {
        match new.sharing.get_mut(path) {
            Some(sh) if sh.role == "member" && !sh.room.is_empty() => forget.push((path.clone(), std::mem::take(&mut sh.room))),
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
        return Err("Only the campaign's owner can do that.".into());
    }
    let secrets = load_room(&sh.room)?;
    Ok((sh.server, sh.room, secrets))
}

/// A campaign whose members you manage (you own it, or the server says you're a DM who manages): (server, room,
/// token, whether you're the owner). The server checks every request again.
fn managed(app: &AppHandle, path: &str) -> Result<(String, String, Secrets, bool), String> {
    let (_, sh) = campaign(app, path)?;
    let owner = sh.role == "owner";
    let manages = owner || sh.access.as_ref().is_some_and(|a| a.role == Role::Dm && a.manage);
    if !manages || sh.room.is_empty() || sh.removed {
        return Err("Only the campaign's owner, or a DM the owner lets manage players, can do that.".into());
    }
    let secrets = load_room(&sh.room)?;
    Ok((sh.server, sh.room, secrets, owner))
}

/// "player" or "dm" from a window.
fn role_from(role: &str) -> Result<Role, String> {
    match role {
        "player" => Ok(Role::Player),
        "dm" => Ok(Role::Dm),
        _ => Err("A player or a DM.".into()),
    }
}

/// Share: makes the campaign's room on the sync server, keeps its key, writes the campaign's name for joiners, and
/// starts syncing (the first sync uploads everything, open campaign or not). Err("create_key_required") asks the window
/// for the server's creation key, which is then remembered for that server only.
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
    let _edit = lock(&EDIT);
    let mut s = current_settings(&app);
    s.sharing.insert(path.clone(), Sharing { server, room, role: "owner".into(), removed: false, ..s.sharing.get(&path).cloned().unwrap_or_default() });
    store_settings(&app, &s)
}

/// Invite players or DMs (`manage`: a DM who may manage players; the owner's to grant): one-time links, each made now
/// and shown once. The key is in the part after #.
#[tauri::command]
pub(crate) async fn sync_invite(app: AppHandle, path: String, count: u32, role: String, manage: bool) -> Result<Vec<String>, String> {
    if !(1..=10).contains(&count) {
        return Err("Make between 1 and 10 invites at a time.".into());
    }
    let role = role_from(&role)?;
    let (server, room, (key, token), _) = managed(&app, &path)?;
    off_main(move || {
        let mut links = Vec::new();
        for _ in 0..count {
            let invite = sync::create_invite(&server, &room, &token, role, manage && role == Role::Dm).map_err(|e| e.to_string())?;
            links.push(Invite { server: server.clone(), room: room.clone(), invite: invite.invite, key: *key }.link());
        }
        Ok(links)
    })
    .await
}

/// Re-invite a member onto a new computer: a one-time link that makes whoever redeems it that member (role and
/// private notes), signing out their old computer.
#[tauri::command]
pub(crate) async fn sync_reinvite(app: AppHandle, path: String, member_id: String) -> Result<String, String> {
    if !is_id(&member_id) {
        return Err("That isn't a player.".into());
    }
    let (server, room, (key, token), _) = managed(&app, &path)?;
    off_main(move || {
        let invite = sync::reinvite(&server, &room, &token, &member_id).map_err(|e| e.to_string())?;
        Ok(Invite { server, room, invite: invite.invite, key: *key }.link())
    })
    .await
}

/// The owner makes a member a player or a DM (`manage`: a DM who may manage players).
#[tauri::command]
pub(crate) async fn sync_set_role(app: AppHandle, path: String, member_id: String, role: String, manage: bool) -> Result<(), String> {
    if !is_id(&member_id) {
        return Err("That isn't a player.".into());
    }
    let role = role_from(&role)?;
    let (server, room, (_, token)) = owned(&app, &path)?;
    off_main(move || sync::update_member(&server, &room, &token, &member_id, role, manage && role == Role::Dm).map_err(|e| e.to_string())).await
}

/// The owner decides who reads private notes: the player only, or the player and the DM. Kept in the campaign's
/// settings at once, so the owner's own labels are right before the server's next word.
#[tauri::command]
pub(crate) async fn sync_set_dm_reads(app: AppHandle, path: String, on: bool) -> Result<(), String> {
    let (server, room, (_, token)) = owned(&app, &path)?;
    off_main(move || sync::set_dm_reads_private(&server, &room, &token, on).map_err(|e| e.to_string())).await?;
    let _edit = lock(&EDIT);
    let mut s = current_settings(&app);
    if let Some(sh) = s.sharing.get_mut(&path) {
        let access = Access { dm_reads_private: on, ..sh.access.clone().unwrap_or(Access { role: Role::Owner, ..Access::default() }) };
        if note_access(&mut s, &path, &access) {
            store_settings(&app, &s)?;
        }
    }
    Ok(())
}

/// The owner says they're also the campaign's DM (then they read private notes as a DM does, when the campaign allows
/// it), or that they aren't. Needs the member id the server gave this computer, so the campaign must have connected.
#[tauri::command]
pub(crate) async fn sync_set_owner_dm(app: AppHandle, path: String, on: bool) -> Result<(), String> {
    let me = campaign(&app, &path)?.1.access.map(|a| a.member_id).filter(|id| is_id(id));
    let me = me.ok_or("Wait until the campaign has connected to the sync server, then try again.")?;
    let (server, room, (_, token)) = owned(&app, &path)?;
    off_main(move || sync::set_owner_is_dm(&server, &room, &token, &me, on).map_err(|e| e.to_string())).await?;
    let _edit = lock(&EDIT);
    let mut s = current_settings(&app);
    if let Some(access) = s.sharing.get(&path).and_then(|sh| sh.access.clone()) {
        if note_access(&mut s, &path, &Access { owner_is_dm: on, ..access }) {
            store_settings(&app, &s)?;
        }
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PendingInvite {
    invite: String,
    expires: i64,
    role: &'static str,
    manage: bool,
    /// A re-invite of an existing member.
    reinvite: bool,
}

fn role_name(role: Role) -> &'static str {
    match role {
        Role::Owner => "owner",
        Role::Dm => "dm",
        Role::Player => "player",
    }
}

#[tauri::command]
pub(crate) async fn sync_invites(app: AppHandle, path: String) -> Result<Vec<PendingInvite>, String> {
    let (server, room, (_, token), _) = managed(&app, &path)?;
    let list = off_main(move || sync::list_invites(&server, &room, &token).map_err(|e| e.to_string())).await?;
    Ok(list
        .into_iter()
        .filter(|i| is_id(&i.invite))
        .map(|i| PendingInvite { invite: i.invite, expires: i.expires, role: role_name(i.role), manage: i.manage, reinvite: i.member_id.is_some() })
        .collect())
}

#[tauri::command]
pub(crate) async fn sync_cancel_invite(app: AppHandle, path: String, invite: String) -> Result<(), String> {
    if !is_id(&invite) {
        return Err("That isn't an invite.".into());
    }
    let (server, room, (_, token), _) = managed(&app, &path)?;
    off_main(move || sync::cancel_invite(&server, &room, &token, &invite).map_err(|e| e.to_string())).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Player {
    member_id: String,
    /// Their PC's name, decrypted; "" when they haven't picked one yet.
    name: String,
    owner: bool,
    /// "owner", "dm" or "player".
    role: &'static str,
    manage: bool,
    /// The owner said they're also the DM.
    owner_is_dm: bool,
    /// It's you (as the server told this computer).
    you: bool,
    created: i64,
    last_seen: Option<i64>,
}

#[tauri::command]
pub(crate) async fn sync_members(app: AppHandle, path: String) -> Result<Vec<Player>, String> {
    let me = campaign(&app, &path)?.1.access.map(|a| a.member_id).unwrap_or_default();
    let (server, room, (key, token), _) = managed(&app, &path)?;
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
            role: role_name(m.role),
            manage: m.manage,
            owner_is_dm: m.owner_is_dm,
            you: m.member_id == me || (me.is_empty() && m.role == Role::Owner),
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
    let (server, room, (_, token), _) = managed(&app, &path)?;
    off_main(move || sync::remove_member(&server, &room, &token, &member_id).map_err(|e| e.to_string())).await
}

/// Makes a campaign yours alone again after Stop sharing or Leave: not shared, no room (only your PC choice stays),
/// its secrets, sync state and DM copies gone from this computer. Its files stay.
async fn local_only(app: &AppHandle, path: &str, room: String) -> Result<(), String> {
    {
        let _edit = lock(&EDIT);
        let mut s = current_settings(app);
        if let Some(sh) = s.sharing.get_mut(path) {
            *sh = Sharing { me: std::mem::take(&mut sh.me), ..Sharing::default() };
        }
        store_settings(app, &s)?; // stops an engine another settings change started meanwhile (see stop_for)
    }
    lock(&SNAPSHOTS).remove(path);
    // Its last state save must not bring back the state file forgotten here, nor its last change a DM copy.
    stopped(path, Duration::MAX).await;
    let (app, path) = (app.clone(), path.to_string());
    let _ = tauri::async_runtime::spawn_blocking(move || {
        forget_room(&app, &room);
        sync::drop_dm_copies(Path::new(&path));
    })
    .await;
    Ok(())
}

/// Runs a request that ends this computer's place in a room (Stop sharing, Leave): the campaign's engine stops first, so it
/// neither sees the room close nor writes its state after the cleanup; a refusal starts it again. A 401 means the room
/// or this member is already gone, which is what was asked.
async fn end_room(app: &AppHandle, path: &str, request: impl FnOnce() -> Result<(), sync::ApiError> + Send + 'static) -> Result<(), String> {
    stop_for(path).await; // still stopping after a while: local_only waits for it before it cleans up
    let result = off_main(move || match request() {
        Err(e) if e.status != 401 => Err(e.to_string()),
        _ => Ok(()),
    })
    .await;
    if result.is_err() {
        restart(app);
    }
    result
}

/// Stop sharing and Leave can't be undone, so only the settings window (which shows no notes from the party) asks.
fn settings_window(window: &tauri::Window) -> Result<(), String> {
    (window.label() == "settings").then_some(()).ok_or_else(|| "Do this in Settings.".to_string())
}

/// Stop sharing (the owner): deletes the room from the server, with every file in it (players' private notes too),
/// and closes everyone's connection; players keep their copies and lose access. Here the campaign becomes yours alone.
#[tauri::command]
pub(crate) async fn sync_stop_sharing(app: AppHandle, window: tauri::Window, path: String) -> Result<(), String> {
    settings_window(&window)?;
    let (server, room, (_, token)) = owned(&app, &path)?;
    let r = room.clone();
    end_room(&app, &path, move || sync::delete_room(&server, &r, &token)).await?;
    local_only(&app, &path, room).await
}

/// Leave (anyone but the owner): the server forgets this member and their private notes; the copy here stays and
/// becomes yours alone.
#[tauri::command]
pub(crate) async fn sync_leave(app: AppHandle, window: tauri::Window, path: String) -> Result<(), String> {
    settings_window(&window)?;
    let (_, sh) = campaign(&app, &path)?;
    if sh.role == "owner" {
        return Err("The campaign's owner can't leave it. Stop sharing it instead.".into());
    }
    if sh.room.is_empty() || sh.removed {
        return Err("This campaign doesn't sync anymore.".into());
    }
    let room = sh.room.clone();
    let r = room.clone();
    let token = off_main(move || load_room(&r)).await?.1;
    let (server, r) = (sh.server.clone(), room.clone());
    end_room(&app, &path, move || sync::leave_room(&server, &r, &token)).await?;
    local_only(&app, &path, room).await
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

/// How long Join waits for the campaign's name (the owner's first upload) before asking you for the folder's name.
const NAME_WAIT: Duration = Duration::from_secs(60);

/// What Join is doing, for its dialog (the "sync-join" event): waiting for the owner's notes, or joining the campaign
/// of that name (from the network: the window shows it as text only).
#[derive(Serialize, Clone)]
struct JoinProgress {
    waiting: bool,
    name: String,
}

/// Join a shared campaign: redeems the invite for a token of your own, waits for the campaign's name (or takes `name`,
/// which the window asks for when it doesn't come in time: Err("name_required")), makes the campaign's folder in the
/// Lorekeeper folder (" 2" and so on when that's taken) and adds it, which starts the download. The window then opens
/// it, shows the download's progress, and asks which PC you play. Returns the new folder.
#[tauri::command]
pub(crate) async fn sync_join(app: AppHandle, link: String, name: Option<String>) -> Result<String, String> {
    let invite = Invite::parse(&link).map_err(|e| format!("That isn't a Lorekeeper invite link ({e})."))?;
    if current_settings(&app).sharing.values().any(|sh| sh.room == invite.room) {
        return Err("You're already in this campaign.".into());
    }
    let given = match name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => Some(sync::folder_name(n).ok_or("That can't be a folder's name: leave out / \\ : * ? \" < > | and a dot at the start.")?),
        None => None,
    };
    let (server, room, key) = (invite.server.clone(), invite.room.clone(), Zeroizing::new(invite.key));
    let token = off_main({
        let (server, room, invite_id, key) = (server.clone(), room.clone(), invite.invite.clone(), key.clone());
        move || {
            // Redeemed already, by a join that stopped to ask for the folder's name: the invite is used up, and this
            // computer holds the token it gave.
            if let Ok((saved, token)) = load_room(&room) {
                if *saved == *key {
                    return Ok(token);
                }
            }
            let redeemed = sync::redeem(&server, &room, &invite_id, "").map_err(|e| e.to_string())?;
            save_room(&room, &key, &redeemed.token)?;
            Ok(Zeroizing::new(redeemed.token))
        }
    })
    .await?;
    drop(invite);
    let name = match given {
        Some(name) => name,
        None => {
            let waiting = {
                let app = app.clone();
                move || {
                    let _ = app.emit("sync-join", JoinProgress { waiting: true, name: String::new() });
                }
            };
            match sync::wait_for_name(&server, &room, &key, &token, NAME_WAIT, waiting).await {
                Ok(name) => name,
                Err(sync::NoName::Missing) => return Err("name_required".into()),
                // Redeemed, so joining again won't need a new invite (see above).
                Err(sync::NoName::Unreachable) => return Err("Can't reach the sync server. Check your internet connection, then click Join again.".into()),
            }
        }
    };
    let _ = app.emit("sync-join", JoinProgress { waiting: false, name: name.clone() });
    let library = library_dir(&app);
    let _edit = lock(&EDIT);
    let s = current_settings(&app);
    // The first free folder whose name also passes the campaign checks (names and backup names must differ).
    let with = |p: &Path| {
        let mut s = s.clone();
        let path = p.to_string_lossy().into_owned();
        s.campaigns.push(path.clone());
        s.sharing.insert(path, Sharing { shared: true, server: server.clone(), room: room.clone(), role: "member".into(), ..Sharing::default() });
        s
    };
    // Never a folder the Lorekeeper folder uses itself (its Templates/ is every campaign's), in any case.
    let taken = |p: &Path| p.exists() || crate::VAULT_FOLDERS.iter().any(|f| p.file_name().is_some_and(|n| sync::folded(&n.to_string_lossy()) == sync::folded(f)));
    let s = (1..=100)
        .map(|n| library.join(if n == 1 { name.clone() } else { format!("{name} {n}") }))
        .filter(|p| !taken(p))
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
        assert!(guard_settings(&old, &mut new).is_empty());
        assert_eq!(new.sharing["/a"], synced("owner", "rooma"));
        assert_eq!(new.sharing["/b"], Sharing { me: "PCs/Arn.md".into(), ..synced("member", "roomb") }, "only shared and me are the window's");

        // Removing a joined campaign leaves its room; removing your own keeps it (the owner token can't be replaced).
        let mut new = Settings { campaigns: vec!["/c".into()], ..old.clone() };
        assert_eq!(guard_settings(&old, &mut new), [("/b".to_string(), "roomb".to_string())]);
        assert_eq!(new.sharing["/b"].room, "");
        assert_eq!(new.sharing["/a"].room, "rooma");

        // A window that hadn't heard of a campaign (no entry for it) can't drop it or leave it.
        let mut stale = Settings { campaigns: vec!["/a".into(), "/b".into()], sharing: old.sharing.clone(), ..old.clone() };
        stale.sharing.remove("/c");
        assert!(guard_settings(&old, &mut stale).is_empty());
        assert!(stale.campaigns.contains(&"/c".to_string()));
        assert_eq!(stale.sharing["/c"], synced("member", "roomc"));
    }

    #[test]
    fn a_refused_token_leaves_a_joined_campaign_but_keeps_the_owners_secrets() {
        let mut s = Settings {
            sharing: [("/a".into(), synced("owner", "rooma")), ("/b".into(), synced("member", "roomb"))].into(),
            ..Settings::default()
        };
        assert_eq!(leave(&mut s, "/a", &Status::Removed), None, "a server can't remove the owner: nothing is forgotten");
        assert_eq!(leave(&mut s, "/a", &Status::Deleted), None, "nor delete the room under them");
        assert_eq!(s.sharing["/a"], synced("owner", "rooma"));
        assert_eq!(leave(&mut s, "/b", &Status::Replaced).as_deref(), Some("roomb"));
        assert_eq!(s.sharing["/b"], Sharing { removed: true, replaced: true, ..synced("member", "") });
        assert_eq!(leave(&mut s, "/missing", &Status::Removed), None);
        assert_eq!(leave(&mut s, "/b", &Status::Deleted), None, "left already: a late close changes nothing");
        assert!(!s.sharing["/b"].unshared);
        // The owner stopped sharing (4003): the member's copy stays, and the window can say why.
        s.sharing.insert("/c".into(), synced("member", "roomc"));
        assert_eq!(leave(&mut s, "/c", &Status::Deleted).as_deref(), Some("roomc"));
        assert_eq!(s.sharing["/c"], Sharing { removed: true, unshared: true, ..synced("member", "") });
        // Only the engine says so: a window can neither raise nor clear it.
        let old = Settings { campaigns: vec!["/b".into(), "/c".into()], ..s.clone() };
        let mut new = old.clone();
        new.sharing.get_mut("/c").unwrap().unshared = false;
        new.sharing.get_mut("/b").unwrap().unshared = true;
        guard_settings(&old, &mut new);
        assert!(new.sharing["/c"].unshared && !new.sharing["/b"].unshared);
    }

    /// What the server says about you is the engine's to set: a window can't claim a role, manage or the setting,
    /// and can only clear the notice. A change of the setting raises the notice for members, not for the owner.
    #[test]
    fn access_comes_from_the_server_only() {
        let access = |role, on| Access { member_id: "m".into(), role, manage: false, owner_is_dm: false, dm_reads_private: on };
        let mut s = Settings { sharing: [("/a".into(), synced("member", "rooma")), ("/o".into(), synced("owner", "roomo"))].into(), ..Settings::default() };
        assert!(note_access(&mut s, "/a", &access(Role::Player, false)));
        assert!(!note_access(&mut s, "/a", &access(Role::Player, false)), "unchanged");
        assert!(!s.sharing["/a"].private_notice, "the first word isn't a change");
        assert!(note_access(&mut s, "/a", &access(Role::Player, true)));
        assert!(s.sharing["/a"].private_notice);
        note_access(&mut s, "/o", &access(Role::Owner, false));
        note_access(&mut s, "/o", &access(Role::Owner, true));
        assert!(!s.sharing["/o"].private_notice, "the owner made the change");

        let old = s.clone();
        let mut new = s.clone();
        let forged = Access { role: Role::Dm, manage: true, ..access(Role::Dm, false) };
        new.sharing.get_mut("/a").unwrap().access = Some(forged);
        new.sharing.get_mut("/o").unwrap().private_notice = true;
        new.sharing.get_mut("/a").unwrap().replaced = true;
        guard_settings(&old, &mut new);
        assert_eq!(new.sharing["/a"].access, Some(access(Role::Player, true)));
        assert!(!new.sharing["/a"].replaced && !new.sharing["/o"].private_notice);
        new.sharing.get_mut("/a").unwrap().private_notice = false;
        guard_settings(&old, &mut new);
        assert!(!new.sharing["/a"].private_notice, "a window clears the notice");
    }

    /// Joining from a DM's invite skips the character question: I play becomes "I'm the DM (no character)".
    #[test]
    fn a_dm_invite_plays_no_character() {
        let access = |role| Access { member_id: "m".into(), role, manage: false, owner_is_dm: false, dm_reads_private: false };
        let picked = Sharing { me: "PCs/Arn.md".into(), ..synced("member", "roomc") };
        let mut s = Settings { sharing: [("/dm".into(), synced("member", "rooma")), ("/p".into(), synced("member", "roomb")), ("/c".into(), picked), ("/o".into(), synced("owner", "roomo"))].into(), ..Settings::default() };
        note_access(&mut s, "/dm", &access(Role::Dm));
        note_access(&mut s, "/p", &access(Role::Player));
        note_access(&mut s, "/c", &access(Role::Dm));
        note_access(&mut s, "/o", &access(Role::Owner));
        assert_eq!(s.sharing["/dm"].me, DM_ME);
        assert_eq!(s.sharing["/p"].me, "", "a player picks their character");
        assert_eq!(s.sharing["/c"].me, "PCs/Arn.md", "a character already picked stays");
        assert_eq!(s.sharing["/o"].me, "");
    }

    /// I play "I'm the DM (no character)" is for the owner and DMs: a player's (from before, or kept from when they
    /// were a DM) is cleared, so they pick a character, even when the server says nothing new.
    #[test]
    fn a_player_never_plays_the_dm() {
        let access = |role| Access { member_id: "m".into(), role, manage: false, owner_is_dm: false, dm_reads_private: false };
        let dm = |role: &str, room: &str| Sharing { me: DM_ME.into(), ..synced(role, room) };
        let known = Sharing { access: Some(access(Role::Player)), ..dm("member", "roomk") };
        let mut s = Settings { sharing: [("/p".into(), dm("member", "rooma")), ("/d".into(), dm("member", "roomb")), ("/o".into(), dm("owner", "roomo")), ("/k".into(), known)].into(), ..Settings::default() };
        assert!(note_access(&mut s, "/p", &access(Role::Player)));
        note_access(&mut s, "/d", &access(Role::Dm));
        note_access(&mut s, "/o", &access(Role::Owner));
        assert!(note_access(&mut s, "/k", &access(Role::Player)), "the same access as before still clears it");
        assert_eq!(s.sharing["/p"].me, "", "a player picks their character");
        assert_eq!(s.sharing["/k"].me, "");
        assert_eq!(s.sharing["/d"].me, DM_ME, "a DM keeps it");
        assert_eq!(s.sharing["/o"].me, DM_ME, "the owner may run the game without a character");
        assert!(!note_access(&mut s, "/k", &access(Role::Player)), "once");
    }

    /// Every shared campaign syncs, open or not (so Share uploads a campaign that isn't open), each with its own PC's
    /// name; switching campaigns changes no engine, only which goes first; past MAX_ENGINES the rest wait.
    #[test]
    fn every_shared_campaign_syncs_up_to_the_limit() {
        let mine = |room: &str, me: &str| Sharing { me: me.into(), ..synced("owner", room) };
        let mut s = Settings {
            vault_path: "/open".into(),
            campaigns: ["/open", "/a", "/b", "/unshared", "/left", "/new"].map(String::from).to_vec(),
            sharing: [
                ("/a".into(), mine("rooma", "PCs/Arn.md")),
                ("/b".into(), Sharing { me: "PCs/Bo.md".into(), ..synced("member", "roomb") }),
                ("/unshared".into(), Sharing { shared: false, ..synced("owner", "roomu") }),
                ("/left".into(), Sharing { removed: true, ..synced("member", "") }),
                ("/new".into(), Sharing { shared: true, ..Sharing::default() }), // turned on, not shared yet
                ("/gone".into(), synced("owner", "roomg")), // removed from the list: its secrets stay, it doesn't sync
            ]
            .into(),
            ..Settings::default()
        };
        let (want, waiting) = targets(&s);
        let names: Vec<(&str, &str, &str)> = want.iter().map(|t| (t.path.as_str(), t.room.as_str(), t.name.as_str())).collect();
        assert_eq!(names, [("/a", "rooma", "Arn"), ("/b", "roomb", "Bo")], "the open campaign isn't shared: the others sync anyway");
        assert!(waiting.is_empty());

        // The open one is shared too: it goes first; switching to /b changes the order, not the engines.
        s.sharing.insert("/open".into(), mine("roomo", "PCs/Ozz.md"));
        let first: Vec<String> = targets(&s).0.into_iter().map(|t| t.path).collect();
        assert_eq!(first, ["/open", "/a", "/b"]);
        s.vault_path = "/b".into();
        let (switched, _) = targets(&s);
        assert_eq!(switched.iter().map(|t| t.path.as_str()).collect::<Vec<_>>(), ["/b", "/open", "/a"]);
        assert!(switched.iter().all(|t| targets(&Settings { vault_path: "/open".into(), ..s.clone() }).0.contains(t)), "same targets");

        // Twelve shared campaigns: eight sync, the open one among them, and four wait.
        for i in 0..9 {
            let path = format!("/more{i}");
            s.campaigns.push(path.clone());
            s.sharing.insert(path, mine(&format!("room{i}"), "PCs/Arn.md"));
        }
        s.vault_path = "/more8".into();
        let (want, waiting) = targets(&s);
        assert_eq!(want.len(), MAX_ENGINES);
        assert_eq!(want[0].path, "/more8", "the open campaign always syncs");
        assert_eq!(waiting, ["/more4", "/more5", "/more6", "/more7"]);
    }

    /// A campaign kept in the Lorekeeper folder itself holds every campaign made or joined since: its engine is told
    /// which folders are theirs, so their notes (private ones too) never go to its party.
    #[test]
    fn a_campaign_holding_others_leaves_their_folders_out() {
        let s = Settings {
            vault_path: "/L".into(),
            campaigns: ["/L", "/L/Strahd", "/L/Deep/Phandelver", "/Lx", "/Elsewhere"].map(String::from).to_vec(),
            sharing: [
                ("/L".into(), synced("owner", "rooml")),
                ("/L/Strahd".into(), synced("member", "rooms")),
                ("/L/Removed".into(), Sharing { shared: true, ..Sharing::default() }), // removed from the list, folder kept
            ]
            .into(),
            backup_names: [("/L/Old".into(), "Old".into())].into(),
            ..Settings::default()
        };
        let (want, _) = targets(&s);
        assert_eq!(want[0].nested, ["Deep/Phandelver", "Old", "Removed", "Strahd"], "not /Lx, a folder next to it");
        assert!(want[1].nested.is_empty());
        let nested = &want[0].nested;
        for inside in ["Strahd", "Strahd/Private/Secrets.md", "strahd/Sessions/Session 1/Arn.md", "Deep/Phandelver/NPCs/Vex.md"] {
            assert!(sync::in_nested(nested, inside), "{inside}");
        }
        for outside in ["Strahdx/Page.md", "Strahd.md", "Deep/Page.md", "Private/Strahd/x.md"] {
            assert!(!sync::in_nested(nested, outside), "{outside}");
        }
    }

    /// An engine told to stop is remembered until it ended: stop_for says when it didn't in time (Delete campaign then
    /// deletes nothing), the cleanup after Leave or Stop sharing waits for it, and the next start for its campaign too.
    #[tokio::test]
    async fn a_stopping_engine_is_waited_for() {
        let (tx, done) = tokio::sync::watch::channel(false);
        lock(&STOPPING).insert("/busy".into(), done);
        assert!(!stopped("/busy", Duration::from_millis(20)).await, "still running");
        tx.send(true).unwrap();
        assert!(stopped("/busy", Duration::MAX).await);
        assert!(stop_for("/never-started").await);
        drop(tx);
        let (tx, done) = tokio::sync::watch::channel(false);
        lock(&STOPPING).insert("/gone".into(), done);
        drop(tx); // a start that gave up: its sender dropped
        assert!(stopped("/gone", Duration::MAX).await);
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
